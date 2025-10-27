//! Shared fixtures and helpers for the fingermouse end-to-end tests.

use anyhow::{anyhow, bail, Context, Result};
use assert_cmd::cargo::CommandCargoExt;
use std::fs;
use std::net::SocketAddr;
use std::process::{Child, Command, Output, Stdio};
use std::path::Path;
use tempfile::{Builder, TempDir};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream as TokioTcpStream;
use tokio::time::{Duration, Instant, sleep, timeout};

pub(crate) const USER_MISSING: &str = "fingermouse: no matching record";
pub(crate) const HOST_MISMATCH: &str = "fingermouse: host not served here";
pub(crate) const GENERIC_ERROR: &str = "fingermouse: request refused";
const SERVER_WARMUP_TIMEOUT: Duration = Duration::from_secs(15);
const SERVER_RETRY_DELAY: Duration = Duration::from_millis(50);
pub(crate) const CONNECTION_TIMEOUT: Duration = Duration::from_secs(3);
pub(crate) const MAX_REQUEST_BYTES: usize = 128;
const MAX_SERVER_START_ATTEMPTS: usize = 5;

/// Manages the server process lifecycle for the duration of a test.
pub(crate) struct TestContext {
    server_process: Child,
    server_addr: SocketAddr,
    _data_dir: TempDir,
    max_request_bytes: usize,
}

impl TestContext {
    pub(crate) const fn address(&self) -> SocketAddr {
        self.server_addr
    }

    pub(crate) const fn max_request_bytes(&self) -> usize {
        self.max_request_bytes
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        if let Ok(Some(_status)) = self.server_process.try_wait() {
            return;
        }

        if let Err(err) = self.server_process.kill() {
            drop(err);
        }
        if let Err(err) = self.server_process.wait() {
            drop(err);
        }
    }
}

/// Spawn the fingermouse server with a temporary datastore.
pub(crate) async fn setup_server() -> Result<TestContext> {
    let data_dir = initialise_datastore()?;
    let root_path = data_dir.path();
    let mut last_failure: Option<String> = None;
    for attempt in 1..=MAX_SERVER_START_ATTEMPTS {
        let server_addr = reserve_server_addr()?;
        let mut server_process = spawn_server(root_path, server_addr)?;

        match wait_for_server(&mut server_process, server_addr).await? {
            ServerReadyState::Ready => {
                return Ok(TestContext {
                    server_process,
                    server_addr,
                    _data_dir: data_dir,
                    max_request_bytes: MAX_REQUEST_BYTES,
                });
            }
            ServerReadyState::ExitedEarly => {
                let output = server_process
                    .wait_with_output()
                    .context("failed to collect server output")?;
                if is_addr_in_use(&output) && attempt < MAX_SERVER_START_ATTEMPTS {
                    last_failure = Some(describe_startup_failure(&output));
                    sleep(SERVER_RETRY_DELAY).await;
                    continue;
                }
                bail!(describe_startup_failure(&output));
            }
            ServerReadyState::TimedOut => {
                let _ = server_process.kill();
                let output = server_process
                    .wait_with_output()
                    .context("failed to collect server output after timeout")?;
                bail!(format!(
                    "server did not become ready within {SERVER_WARMUP_TIMEOUT:?}\n{}",
                    describe_startup_failure(&output)
                ));
            }
        }
    }

    let summary = last_failure.unwrap_or_else(|| "server failed to start for an unknown reason".to_owned());
    bail!(format!(
        "exhausted {MAX_SERVER_START_ATTEMPTS} attempts to start fingermouse\n{summary}"
    ));
}

fn initialise_datastore() -> Result<TempDir> {
    let data_dir = Builder::new()
        .prefix("fingermouse-e2e-")
        .tempdir()
        .context("failed to create temporary data directory")?;
    seed_datastore(data_dir.path())?;
    Ok(data_dir)
}

fn seed_datastore(root_path: &Path) -> Result<()> {
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
    Ok(())
}

fn reserve_server_addr() -> Result<SocketAddr> {
    let port = portpicker::pick_unused_port().ok_or_else(|| anyhow!("no free ports"))?;
    Ok(format!("127.0.0.1:{port}").parse()?)
}

fn spawn_server(root_path: &Path, server_addr: SocketAddr) -> Result<Child> {
    let mut command = build_server_command(root_path, server_addr)?;
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn fingermouse")
}

fn build_server_command(root_path: &Path, server_addr: SocketAddr) -> Result<Command> {
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
        .arg(MAX_REQUEST_BYTES.to_string());
    Ok(cmd)
}

enum ServerReadyState {
    Ready,
    ExitedEarly,
    TimedOut,
}

async fn wait_for_server(child: &mut Child, addr: SocketAddr) -> Result<ServerReadyState> {
    let deadline = Instant::now() + SERVER_WARMUP_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(_status) = child
            .try_wait()
            .context("failed to poll fingermouse process state")?
        {
            return Ok(ServerReadyState::ExitedEarly);
        }
        match timeout(CONNECTION_TIMEOUT, TokioTcpStream::connect(addr)).await {
            Ok(Ok(mut stream)) => {
                stream
                    .shutdown()
                    .await
                    .context("failed to close readiness probe connection")?;
                return Ok(ServerReadyState::Ready);
            }
            Ok(Err(_)) | Err(_) => sleep(SERVER_RETRY_DELAY).await,
        }
    }
    Ok(ServerReadyState::TimedOut)
}

fn describe_startup_failure(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("fingermouse exited during startup\nstdout:\n{stdout}\nstderr:\n{stderr}")
}

fn is_addr_in_use(output: &Output) -> bool {
    let haystacks = [
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ];
    haystacks
        .iter()
        .any(|content| content.contains("address already in use") || content.contains("AddrInUse"))
}

/// Send a finger query to the running server instance.
pub(crate) async fn query_server(addr: SocketAddr, query: &str) -> Result<String> {
    let connect_attempt = timeout(CONNECTION_TIMEOUT, TokioTcpStream::connect(addr))
        .await
        .context("timed out connecting to server")?;
    let mut stream = connect_attempt.context("failed to connect to server")?;
    stream
        .write_all(query.as_bytes())
        .await
        .context("failed to send query")?;
    stream
        .write_all(b"\r\n")
        .await
        .context("failed to send CRLF terminator")?;
    stream
        .shutdown()
        .await
        .context("failed to close write half")?;

    let mut buffer = Vec::new();
    let read_attempt = timeout(CONNECTION_TIMEOUT, stream.read_to_end(&mut buffer))
        .await
        .context("timed out waiting for server response")?;
    read_attempt.context("failed to read response")?;
    String::from_utf8(buffer).context("response was not valid UTF-8")
}
