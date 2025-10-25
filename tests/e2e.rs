//! End-to-end tests for the fingermouse daemon binary.

use anyhow::{Context, Result, anyhow, bail};
use assert_cmd::cargo::CommandCargoExt;
use predicates::prelude::*;
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

        if let Err(err) = self.server_process.kill() {
            eprintln!("failed to kill fingermouse process: {err}");
        }

        let _ = self.server_process.wait();
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
                let _ = stream.shutdown(Shutdown::Both);
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
fn test_query_user_direct_connection() -> Result<()> {
    let ctx = setup_server()?;

    let response_alice = query_server(ctx.server_addr, "alice")?;
    let alice_predicate =
        predicates::str::contains("User: alice").and(predicates::str::contains("Alice Example"));
    assert!(
        alice_predicate.eval(&response_alice),
        "unexpected response: {response_alice:?}"
    );
    assert!(
        response_alice.contains("Email: alice@test.host"),
        "expected email field in response: {response_alice:?}"
    );
    assert!(
        !response_alice.contains("Plan:"),
        "non-verbose query should not include plan: {response_alice:?}"
    );

    let response_alice_verbose = query_server(ctx.server_addr, "/W alice@test.host")?;
    assert!(response_alice_verbose.contains("User: alice"));
    assert!(response_alice_verbose.contains("Plan:"));
    assert!(response_alice_verbose.contains("Alice's Plan"));
    assert!(response_alice_verbose.contains("Line 2"));

    let response_bob_verbose = query_server(ctx.server_addr, "/W bob")?;
    assert!(response_bob_verbose.contains("User: bob"));
    assert!(response_bob_verbose.contains("(no plan)"));

    let response_missing = query_server(ctx.server_addr, "charlie")?;
    assert!(response_missing.contains(USER_MISSING));

    let response_bad_host = query_server(ctx.server_addr, "alice@wrong.host")?;
    assert!(response_bad_host.contains(HOST_MISMATCH));

    Ok(())
}

struct TcpProxy {
    local_addr: SocketAddr,
    task: Option<JoinHandle<()>>,
}

impl TcpProxy {
    fn address(&self) -> SocketAddr {
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
        if let Err(err) = run_passthrough_proxy(listener, upstream).await {
            eprintln!("passthrough proxy failed: {err}");
        }
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
            Err(err) => {
                eprintln!("proxy failed to connect to upstream: {err}");
                continue;
            }
        };
        tokio::spawn(async move {
            let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
        });
    }
}

async fn spawn_fault_injecting_proxy(upstream: SocketAddr, payload: Vec<u8>) -> Result<TcpProxy> {
    let listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind fault proxy listener")?;
    let local_addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        if let Err(err) = run_fault_injecting_proxy(listener, upstream, payload).await {
            eprintln!("fault proxy failed: {err}");
        }
    });
    Ok(TcpProxy {
        local_addr,
        task: Some(task),
    })
}

async fn run_fault_injecting_proxy(
    listener: TokioTcpListener,
    upstream: SocketAddr,
    payload: Vec<u8>,
) -> Result<()> {
    let (mut client, _) = listener
        .accept()
        .await
        .context("failed to accept client connection")?;
    let mut server = TokioTcpStream::connect(upstream)
        .await
        .context("failed to connect to upstream server")?;

    // Read and discard the client's query to simulate the proxy mutating it.
    let mut buf = [0_u8; 1];
    while let Ok(read) = client.read(&mut buf).await {
        if read == 0 || buf[0] == b'\n' {
            break;
        }
    }

    server
        .write_all(&payload)
        .await
        .context("failed to send injected payload")?;
    server
        .shutdown()
        .await
        .context("failed to close upstream write half")?;

    tokio::io::copy(&mut server, &mut client)
        .await
        .context("failed to relay server response")?;
    client
        .shutdown()
        .await
        .context("failed to close client connection")?;
    Ok(())
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
    Ok(String::from_utf8(buffer).context("response was not valid UTF-8")?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_query_user_via_proxy() -> Result<()> {
    let ctx = setup_server()?;
    let proxy = spawn_passthrough_proxy(ctx.server_addr).await?;

    let response_alice = query_proxy(proxy.address(), "alice@test.host").await?;
    assert!(response_alice.contains("User: alice"));
    assert!(response_alice.contains("Full name: Alice Example"));

    let response_bob = query_proxy(proxy.address(), "/W bob").await?;
    assert!(response_bob.contains("User: bob"));
    assert!(response_bob.contains("(no plan)"));

    proxy.shutdown().await?;
    Ok(())
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

    assert!(
        response.trim().is_empty() || response.contains(GENERIC_ERROR),
        "expected generic error or disconnect, got {response:?}"
    );

    proxy.shutdown().await?;
    Ok(())
}
