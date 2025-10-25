//! Proxy-based tests exercising passthrough and fault injection flows.

use anyhow::{Context, Result, bail, ensure};
use std::net::SocketAddr;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener as TokioTcpListener, TcpStream as TokioTcpStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::common::{CONNECTION_TIMEOUT, GENERIC_ERROR, MAX_REQUEST_BYTES, setup_server};

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
                Err(err) => Err(anyhow::Error::from(err)),
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
            if let Err(_err) = io::copy_bidirectional(&mut inbound, &mut outbound).await {}
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
    let connect_attempt = timeout(CONNECTION_TIMEOUT, TokioTcpStream::connect(upstream))
        .await
        .context("timed out connecting to upstream server")?;
    connect_attempt.context("failed to connect to upstream server")
}

async fn discard_client_query(client: &mut TokioTcpStream) -> Result<()> {
    let mut buf = [0_u8; 1];
    let mut received = 0_usize;
    loop {
        let read = timeout(CONNECTION_TIMEOUT, client.read(&mut buf))
            .await
            .context("timed out waiting for client query")??;
        if read == 0 {
            bail!("client closed before completing query");
        }
        received += read;
        if received > MAX_REQUEST_BYTES {
            bail!("client query exceeded maximum length while awaiting newline");
        }
        if buf[0] == b'\n' {
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
    io::copy(server, client)
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
    let connect_attempt = timeout(CONNECTION_TIMEOUT, TokioTcpStream::connect(addr))
        .await
        .with_context(|| format!("timed out connecting to proxy at {addr}"))?;
    let mut stream =
        connect_attempt.with_context(|| format!("failed to connect to proxy at {addr}"))?;
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
    let read_attempt = timeout(CONNECTION_TIMEOUT, stream.read_to_end(&mut buffer))
        .await
        .context("timed out waiting for proxy response")?;
    read_attempt.context("failed to read proxy response")?;
    String::from_utf8(buffer).context("response was not valid UTF-8")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_query_user_via_proxy() -> Result<()> {
    let ctx = setup_server().await?;
    let proxy = spawn_passthrough_proxy(ctx.address()).await?;

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
    let ctx = setup_server().await?;

    let oversized = vec![b'a'; ctx.max_request_bytes() + 50];
    let mut payload = oversized;
    payload.extend_from_slice(b"\r\n");

    let proxy = spawn_fault_injecting_proxy(ctx.address(), payload).await?;

    let connect_attempt = timeout(CONNECTION_TIMEOUT, TokioTcpStream::connect(proxy.address()))
        .await
        .context("timed out connecting to proxy")?;
    let mut client = connect_attempt.context("failed to connect to proxy")?;
    client
        .write_all(b"alice\r\n")
        .await
        .context("failed to send baseline query to proxy")?;
    client
        .shutdown()
        .await
        .context("failed to close fault-injection client")?;

    let mut buffer = Vec::new();
    let read_attempt = timeout(CONNECTION_TIMEOUT, client.read_to_end(&mut buffer))
        .await
        .context("timed out waiting for fault response")?;
    read_attempt.context("failed to read fault proxy response")?;
    let response = String::from_utf8_lossy(&buffer);

    ensure!(
        response.trim().is_empty() || response.contains(GENERIC_ERROR),
        "expected generic error or disconnect, got {response:?}"
    );

    proxy.shutdown().await
}
