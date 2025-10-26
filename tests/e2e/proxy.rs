//! Proxy-based tests exercising passthrough and fault injection flows.

use anyhow::{bail, ensure, Context, Result};
use rstest::rstest;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{self, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener as TokioTcpListener, TcpStream as TokioTcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

use super::common::{setup_server, CONNECTION_TIMEOUT, GENERIC_ERROR, MAX_REQUEST_BYTES};

struct TcpProxy {
    local_addr: SocketAddr,
    task: Option<JoinHandle<()>>,
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
    cancel: CancellationToken,
}

impl TcpProxy {
    const fn address(&self) -> SocketAddr {
        self.local_addr
    }

    async fn shutdown(self) -> Result<()> {
        self.shutdown_inner().await
    }

    async fn shutdown_inner(mut self) -> Result<()> {
        self.cancel.cancel();
        let mut connections = self.connections.lock().await;
        for handle in connections.drain(..) {
            handle.abort();
        }
        drop(connections);

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
        self.cancel.cancel();
        if let Ok(mut connections) = self.connections.try_lock() {
            for handle in connections.drain(..) {
                handle.abort();
            }
        }
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn spawn_proxy<F, Fut>(
    context_msg: &str,
    runner: F,
) -> Result<TcpProxy>
where
    F: FnOnce(
            TokioTcpListener,
            CancellationToken,
            Arc<Mutex<Vec<JoinHandle<()>>>>,
        ) -> Fut
        + Send
        + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    let listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .context(context_msg.to_owned())?;
    let local_addr = listener.local_addr()?;
    let cancel = CancellationToken::new();
    let connections: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let runner_cancel = cancel.clone();
    let runner_connections = Arc::clone(&connections);
    let task = tokio::spawn(async move {
        if let Err(_err) = runner(listener, runner_cancel, runner_connections).await {}
    });

    Ok(TcpProxy {
        local_addr,
        task: Some(task),
        connections,
        cancel,
    })
}

async fn spawn_passthrough_proxy(upstream: SocketAddr) -> Result<TcpProxy> {
    spawn_proxy(
        "failed to bind proxy listener",
        move |listener, cancel, connections| async move {
            run_passthrough_proxy(listener, upstream, cancel, connections).await
        },
    )
    .await
}

async fn run_passthrough_proxy(
    listener: TokioTcpListener,
    upstream: SocketAddr,
    cancel: CancellationToken,
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            accept_result = listener.accept() => {
                let (mut inbound, _) = accept_result
                    .context("failed to accept client connection")?;

                let mut outbound = match TokioTcpStream::connect(upstream).await {
                    Ok(stream) => stream,
                    Err(_err) => {
                        sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };

                let task_cancel = cancel.clone();
                let handle = tokio::spawn(async move {
                    tokio::select! {
                        _ = task_cancel.cancelled() => {
                            let _ = inbound.shutdown().await;
                            let _ = outbound.shutdown().await;
                        }
                        transfer = io::copy_bidirectional(&mut inbound, &mut outbound) => {
                            if transfer.is_err() {
                                let _ = inbound.shutdown().await;
                                let _ = outbound.shutdown().await;
                            }
                        }
                    }
                });
                connections.lock().await.push(handle);
            }
        }
    }

    Ok(())
}

async fn spawn_fault_injecting_proxy(upstream: SocketAddr, payload: Vec<u8>) -> Result<TcpProxy> {
    spawn_proxy(
        "failed to bind fault proxy listener",
        move |listener, cancel, connections| async move {
            run_fault_injecting_proxy(listener, upstream, payload, cancel, connections).await
        },
    )
    .await
}

async fn connect_upstream_server(upstream: SocketAddr) -> Result<TokioTcpStream> {
    let connect_attempt = timeout(CONNECTION_TIMEOUT, TokioTcpStream::connect(upstream))
        .await
        .context("timed out connecting to upstream server")?;
    connect_attempt.context("failed to connect to upstream server")
}

async fn discard_client_query(client: &mut TokioTcpStream) -> Result<()> {
    let mut reader = BufReader::new(client);
    let mut buf = Vec::with_capacity(128);
    let read = timeout(CONNECTION_TIMEOUT, reader.read_until(b'\n', &mut buf))
        .await
        .context("timed out waiting for client query")??;
    if read == 0 {
        bail!("client closed before completing query");
    }
    if buf.len() > MAX_REQUEST_BYTES {
        bail!("client query exceeded maximum length while awaiting newline");
    }
    Ok(())
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

/// Runs a single fault-injection cycle: accepts one client, discards its query,
/// sends the injected payload upstream, and relays the response. The proxy exits
/// after serving the first client.
async fn run_fault_injecting_proxy(
    listener: TokioTcpListener,
    upstream: SocketAddr,
    payload: Vec<u8>,
    cancel: CancellationToken,
    _connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
) -> Result<()> {
    let mut client = tokio::select! {
        _ = cancel.cancelled() => return Ok(()),
        accept_result = listener.accept() => {
            let (client, _) = accept_result
                .context("failed to accept client connection")?;
            client
        }
    };
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

#[rstest]
#[case("alice@test.host", &["User: alice", "Full name: Alice Example"])]
#[case("/W bob", &["User: bob", "(no plan)"])]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_query_user_via_proxy(
    #[case] query: &str,
    #[case] expectations: &[&str],
) -> Result<()> {
    let ctx = setup_server().await?;
    let proxy = spawn_passthrough_proxy(ctx.address()).await?;

    let response = query_proxy(proxy.address(), query).await?;
    for needle in expectations {
        ensure!(
            response.contains(needle),
            "missing {needle:?} in proxied response: {response:?}"
        );
    }

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

    let trimmed = response.trim();
    ensure!(
        trimmed.is_empty() || response.contains(GENERIC_ERROR),
        "expected generic error or disconnect, got len={} body={response:?}",
        trimmed.len()
    );

    proxy.shutdown().await
}
