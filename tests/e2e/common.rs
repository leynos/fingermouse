//! Shared fixtures and helpers for the fingermouse end-to-end tests.

use anyhow::{Context, Result, anyhow, bail};
use assert_cmd::cargo::CommandCargoExt;
use std::fs;
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use tempfile::{Builder, TempDir};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream as TokioTcpStream;
use tokio::time::{Duration, Instant, sleep, timeout};

pub(crate) const USER_MISSING: &str = "fingermouse: no matching record";
pub(crate) const HOST_MISMATCH: &str = "fingermouse: host not served here";
pub(crate) const GENERIC_ERROR: &str = "fingermouse: request refused";
const SERVER_WARMUP_TIMEOUT: Duration = Duration::from_secs(5);
const SERVER_RETRY_DELAY: Duration = Duration::from_millis(50);
pub(crate) const CONNECTION_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const MAX_REQUEST_BYTES: usize = 128;

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
        .arg(MAX_REQUEST_BYTES.to_string());

    let server_process = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn fingermouse")?;

    wait_for_server(server_addr).await?;

    Ok(TestContext {
        server_process,
        server_addr,
        _data_dir: data_dir,
        max_request_bytes: MAX_REQUEST_BYTES,
    })
}

async fn wait_for_server(addr: SocketAddr) -> Result<()> {
    let deadline = Instant::now() + SERVER_WARMUP_TIMEOUT;
    while Instant::now() < deadline {
        match timeout(CONNECTION_TIMEOUT, TokioTcpStream::connect(addr)).await {
            Ok(Ok(mut stream)) => {
                stream
                    .shutdown()
                    .await
                    .context("failed to close readiness probe connection")?;
                return Ok(());
            }
            Ok(Err(_)) | Err(_) => sleep(SERVER_RETRY_DELAY).await,
        }
    }
    bail!("server did not become ready within {SERVER_WARMUP_TIMEOUT:?}");
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
