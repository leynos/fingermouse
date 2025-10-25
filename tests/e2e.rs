//! End-to-end tests for the fingermouse daemon binary.

use anyhow::{Context, Result, anyhow, bail, ensure};
use assert_cmd::cargo::CommandCargoExt;
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::{Builder, TempDir};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener as TokioTcpListener, TcpStream as TokioTcpStream};
use tokio::task::JoinHandle;

const USER_MISSING: &str = "fingermouse: no matching record";
const HOST_MISMATCH: &str = "fingermouse: host not served here";
const GENERIC_ERROR: &str = "fingermouse: request refused";
const SERVER_WARMUP_TIMEOUT: Duration = Duration::from_secs(5);
const SERVER_RETRY_DELAY: Duration = Duration::from_millis(50);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_REQUEST_BYTES: usize = 128;

/// Manages the server process lifecycle for the duration of a test.
struct TestContext {
    server_process: Child,
    server_addr: SocketAddr,
    _data_dir: TempDir,
    max_request_bytes: usize,
}

impl Drop for TestContext {
    fn drop(&mut self) {
        if let Ok(Some(_status)) = self.server_process.try_wait() {
            return;
        }

        if let Err(_err) = self.server_process.kill() {}
        if self.server_process.wait().is_err() {}
    }
}

/// Spawn the fingermouse server with a temporary datastore.
fn setup_server() -> Result<TestContext> {
    let data_dir = Builder::new()
        .prefix("fingermouse-e2e-")
        .tempdir()
        .context("failed to create temporary data directory")?;
    let root_path = data_dir.path();

    fs::create_dir_all(root_path.join("profiles")).context("failed to create profile directory")?;
    fs::create_dir_all(root_path.join("plans")).context("failed to create plan directory")?;

    let alice_profile = r#"
        username = "alice"
        full_name = "Alice Example"
        email = "alice@test.host"
    "#;
    fs::write(
        root_path.join("profiles/alice.toml"),
        alice_profile.as_bytes(),
    )
    .context("failed to write alice profile")?;
    fs::write(
        root_path.join("plans/alice.plan"),
        b"Alice's Plan\r\nLine 2\r\n",
    )
    .context("failed to write alice plan")?;

    let bob_profile = r#"
        username = "bob"
        full_name = "Bob Test"
    "#;
    fs::write(root_path.join("profiles/bob.toml"), bob_profile.as_bytes())
        .context("failed to write bob profile")?;

    let port = portpicker::pick_unused_port().ok_or_else(|| anyhow!("no free ports"))?;
    let server_addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;

    let mut cmd = Command::cargo_bin("fingermouse")?;
    cmd.arg("--listen")
        .arg(server_addr.to_string())
        .arg("--store-root")
        .arg(root_path)
        .arg("--default-host")
        .arg("test.host")
        .arg("--allowed-hosts")
        .arg("test.host,other.host")
        .arg("--rate-limit")
        .arg("1000")
        .arg("--request-timeout-ms")
        .arg("2000")
        .arg("--max-request-bytes")
        .arg(MAX_REQUEST_BYTES.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let server_process = cmd.spawn().context("failed to spawn fingermouse")?;

    wait_for_server(server_addr)?;

    Ok(TestContext {
        server_process,
        server_addr,
        _data_dir: data_dir,
        max_request_bytes: MAX_REQUEST_BYTES,
    })
}

fn wait_for_server(addr: SocketAddr) -> Result<()> {
    let deadline = Instant::now() + SERVER_WARMUP_TIMEOUT;
    while Instant::now() < deadline {
        match TcpStream::connect(addr) {
            Ok(stream) => {
                stream
                    .shutdown(Shutdown::Both)
                    .context("failed to close readiness probe connection")?;
                return Ok(());
            }
            Err(_) => thread::sleep(SERVER_RETRY_DELAY),
        }
    }
    bail!("server did not become ready within {SERVER_WARMUP_TIMEOUT:?}");
}

fn query_server(addr: SocketAddr, query: &str) -> Result<String> {
    let mut stream = TcpStream::connect_timeout(&addr, CONNECTION_TIMEOUT)
        .with_context(|| format!("failed to connect to {addr}"))?;
    stream
        .set_read_timeout(Some(CONNECTION_TIMEOUT))
        .context("failed to set read timeout")?;
    stream
        .set_write_timeout(Some(CONNECTION_TIMEOUT))
        .context("failed to set write timeout")?;
    stream
        .write_all(query.as_bytes())
        .context("failed to send query")?;
    stream
        .write_all(b"\r\n")
        .context("failed to send CRLF terminator")?;
    stream
        .shutdown(Shutdown::Write)
        .context("failed to close write half")?;

    let mut buffer = String::new();
    stream
        .read_to_string(&mut buffer)
        .context("failed to read response")?;
    Ok(buffer)
}

#[test]
fn test_direct_query_includes_basic_profile_details() -> Result<()> {
    let ctx = setup_server()?;

    let response = query_server(ctx.server_addr, "alice")?;
    ensure!(
        response.contains("User: alice"),
        "missing username in response: {response:?}"
    );
    ensure!(
        response.contains("Full name: Alice Example"),
        "missing full name in response: {response:?}"
    );
    ensure!(
        response.contains("Email: alice@test.host"),
        "missing email field in response: {response:?}"
    );
    ensure!(
        !response.contains("Plan:"),
        "non-verbose query should not include plan: {response:?}"
    );

    Ok(())
}

#[test]
fn test_verbose_query_returns_plan_content() -> Result<()> {
    let ctx = setup_server()?;

    let response = query_server(ctx.server_addr, "/W alice@test.host")?;
    ensure!(
        response.contains("User: alice"),
        "missing username in verbose response: {response:?}"
    );
    ensure!(
        response.contains("Plan:"),
        "verbose response missing plan header: {response:?}"
    );
    ensure!(
        response.contains("Alice's Plan"),
        "verbose response missing plan contents: {response:?}"
    );
    ensure!(
        response.contains("Line 2"),
        "verbose response missing trailing plan line: {response:?}"
    );

    Ok(())
}

#[test]
fn test_verbose_query_reports_missing_plan() -> Result<()> {
    let ctx = setup_server()?;

    let response = query_server(ctx.server_addr, "/W bob")?;
    ensure!(
        response.contains("User: bob"),
        "missing username for bob response: {response:?}"
    );
    ensure!(
        response.contains("(no plan)"),
        "expected missing plan marker: {response:?}"
    );

    Ok(())
}

#[test]
fn test_query_handles_unknown_user_and_host_mismatch() -> Result<()> {
    let ctx = setup_server()?;

    let missing_response = query_server(ctx.server_addr, "charlie")?;
    ensure!(
        missing_response.contains(USER_MISSING),
        "missing user sentinel in response: {missing_response:?}"
    );

    let host_response = query_server(ctx.server_addr, "alice@wrong.host")?;
    ensure!(
        host_response.contains(HOST_MISMATCH),
        "missing host mismatch sentinel in response: {host_response:?}"
    );

    Ok(())
}

struct TcpProxy {
    local_addr: SocketAddr,
    task: Option<JoinHandle<()>>,
}

impl TcpProxy {
    const fn address(&self) -> SocketAddr {
        self.local_addr
    }

    async fn shutdown(self) -> Result<()> {
        self.shutdown_inner().await
    }

    async fn shutdown_inner(mut self) -> Result<()> {
        if let Some(task) = self.task.take() {
            task.abort();
            match task.await {
                Ok(()) => Ok(()),
                Err(err) if err.is_cancelled() => Ok(()),
                Err(err) => Err(anyhow!(err)),
            }
        } else {
            Ok(())
        }
    }
}

impl Drop for TcpProxy {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn spawn_passthrough_proxy(upstream: SocketAddr) -> Result<TcpProxy> {
    let listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind proxy listener")?;
    let local_addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        if let Err(_err) = run_passthrough_proxy(listener, upstream).await {}
    });
    Ok(TcpProxy {
        local_addr,
        task: Some(task),
    })
}

async fn run_passthrough_proxy(listener: TokioTcpListener, upstream: SocketAddr) -> Result<()> {
    loop {
        let (mut inbound, _) = listener
            .accept()
            .await
            .context("failed to accept client connection")?;
        let mut outbound = match TokioTcpStream::connect(upstream).await {
            Ok(stream) => stream,
            Err(_err) => continue,
        };
        tokio::spawn(async move {
            if let Err(_err) = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await {}
        });
    }
}

async fn spawn_fault_injecting_proxy(upstream: SocketAddr, payload: Vec<u8>) -> Result<TcpProxy> {
    let listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind fault proxy listener")?;
    let local_addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        if let Err(_err) = run_fault_injecting_proxy(listener, upstream, payload).await {}
    });
    Ok(TcpProxy {
        local_addr,
        task: Some(task),
    })
}

async fn accept_proxy_client(listener: &TokioTcpListener) -> Result<TokioTcpStream> {
    let (client, _) = listener
        .accept()
        .await
        .context("failed to accept client connection")?;
    Ok(client)
}

async fn connect_upstream_server(upstream: SocketAddr) -> Result<TokioTcpStream> {
    TokioTcpStream::connect(upstream)
        .await
        .context("failed to connect to upstream server")
}

async fn discard_client_query(client: &mut TokioTcpStream) -> Result<()> {
    let mut buf = [0_u8; 1];
    loop {
        let read = client
            .read(&mut buf)
            .await
            .context("failed to read client query")?;
        if read == 0 || buf[0] == b'\n' {
            return Ok(());
        }
    }
}

async fn send_injected_payload(server: &mut TokioTcpStream, payload: &[u8]) -> Result<()> {
    server
        .write_all(payload)
        .await
        .context("failed to send injected payload")?;
    server
        .shutdown()
        .await
        .context("failed to close upstream write half")?;
    Ok(())
}

async fn relay_server_response(
    server: &mut TokioTcpStream,
    client: &mut TokioTcpStream,
) -> Result<()> {
    tokio::io::copy(server, client)
        .await
        .context("failed to relay server response")?;
    client
        .shutdown()
        .await
        .context("failed to close client connection")?;
    Ok(())
}

async fn run_fault_injecting_proxy(
    listener: TokioTcpListener,
    upstream: SocketAddr,
    payload: Vec<u8>,
) -> Result<()> {
    let mut client = accept_proxy_client(&listener).await?;
    let mut server = connect_upstream_server(upstream).await?;

    discard_client_query(&mut client).await?;
    send_injected_payload(&mut server, &payload).await?;
    relay_server_response(&mut server, &mut client).await
}

async fn query_proxy(addr: SocketAddr, query: &str) -> Result<String> {
    let mut stream = TokioTcpStream::connect(addr)
        .await
        .with_context(|| format!("failed to connect to proxy at {addr}"))?;
    stream
        .write_all(query.as_bytes())
        .await
        .context("failed to write proxy request")?;
    stream
        .write_all(b"\r\n")
        .await
        .context("failed to write proxy terminator")?;
    stream
        .shutdown()
        .await
        .context("failed to close proxy stream")?;

    let mut buffer = Vec::new();
    stream
        .read_to_end(&mut buffer)
        .await
        .context("failed to read proxy response")?;
    String::from_utf8(buffer).context("response was not valid UTF-8")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_query_user_via_proxy() -> Result<()> {
    let ctx = setup_server()?;
    let proxy = spawn_passthrough_proxy(ctx.server_addr).await?;

    let response_alice = query_proxy(proxy.address(), "alice@test.host").await?;
    ensure!(
        response_alice.contains("User: alice"),
        "missing username in proxied response: {response_alice:?}"
    );
    ensure!(
        response_alice.contains("Full name: Alice Example"),
        "missing full name in proxied response: {response_alice:?}"
    );

    let response_bob = query_proxy(proxy.address(), "/W bob").await?;
    ensure!(
        response_bob.contains("User: bob"),
        "missing username in proxied response: {response_bob:?}"
    );
    ensure!(
        response_bob.contains("(no plan)"),
        "missing empty plan marker in proxied response: {response_bob:?}"
    );

    proxy.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_server_rejects_large_query() -> Result<()> {
    let ctx = setup_server()?;

    let oversized = vec![b'a'; ctx.max_request_bytes + 50];
    let mut payload = oversized;
    payload.extend_from_slice(b"\r\n");

    let proxy = spawn_fault_injecting_proxy(ctx.server_addr, payload).await?;

    let mut client = TokioTcpStream::connect(proxy.address()).await?;
    client.write_all(b"alice\r\n").await?;
    client.shutdown().await?;

    let mut buffer = Vec::new();
    client.read_to_end(&mut buffer).await?;
    let response = String::from_utf8_lossy(&buffer);

    ensure!(
        response.trim().is_empty() || response.contains(GENERIC_ERROR),
        "expected generic error or disconnect, got {response:?}"
    );

    proxy.shutdown().await
}
