//! Telemetry initialisation and metrics exposition.
use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Handle to the background metrics endpoint.
pub struct MetricsEndpoint {
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl MetricsEndpoint {
    /// Signal the endpoint to stop and wait for it to terminate.
    pub async fn shutdown(mut self) -> Result<()> {
        if self
            .shutdown
            .take()
            .is_some_and(|sender| sender.send(()).is_err())
        {
            warn!("metrics endpoint shutdown channel already closed during join");
        }
        if let Some(join) = self.join.take() {
            join.await
                .map_err(|err| anyhow!("metrics task join failed: {err}"))?;
        }
        Ok(())
    }
}

impl Drop for MetricsEndpoint {
    fn drop(&mut self) {
        if self
            .shutdown
            .take()
            .is_some_and(|sender| sender.send(()).is_err())
        {
            warn!("metrics endpoint shutdown channel already closed on drop");
        }
    }
}

/// Install the global metrics recorder and, when requested, expose Prometheus metrics.
pub async fn install_metrics(listen: Option<SocketAddr>) -> Result<Option<MetricsEndpoint>> {
    let handle = PrometheusBuilder::new()
        .install_recorder()
        .context("failed to install metrics recorder")?;

    let endpoint = match listen {
        Some(addr) => Some(spawn_metrics_endpoint(handle, addr).await?),
        None => None,
    };

    Ok(endpoint)
}

async fn spawn_metrics_endpoint(
    handle: PrometheusHandle,
    addr: SocketAddr,
) -> Result<MetricsEndpoint> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind metrics listener on {addr}"))?;
    info!(address = ?addr, "exposing Prometheus metrics endpoint");

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        if let Err(err) = run_metrics_server(listener, handle, shutdown_rx).await {
            warn!(error = ?err, "metrics endpoint terminated abnormally");
        }
        info!("metrics endpoint stopped");
    });

    Ok(MetricsEndpoint {
        shutdown: Some(shutdown_tx),
        join: Some(join),
    })
}

#[expect(
    clippy::integer_division_remainder_used,
    reason = "tokio::select! macro expands to `%` operations internally"
)]
async fn run_metrics_server(
    listener: TcpListener,
    handle: PrometheusHandle,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => handle_accept(accepted, &handle).await,
        }
    }
    Ok(())
}

async fn handle_accept(
    accepted: std::io::Result<(tokio::net::TcpStream, SocketAddr)>,
    handle: &PrometheusHandle,
) {
    match accepted {
        Ok((mut socket, peer)) => handle_connection(&mut socket, peer, handle).await,
        Err(err) => handle_accept_error(err).await,
    }
}

async fn handle_connection(
    socket: &mut tokio::net::TcpStream,
    peer: SocketAddr,
    handle: &PrometheusHandle,
) {
    if let Err(err) = discard_request(socket).await {
        warn!(peer = ?peer, error = ?err, "failed to read metrics request");
        return;
    }
    if let Err(err) = respond_with_metrics(handle, socket).await {
        warn!(peer = ?peer, error = ?err, "failed to respond with metrics");
    }
}

async fn handle_accept_error(err: std::io::Error) {
    warn!(error = ?err, "metrics accept failed");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}

async fn discard_request(socket: &mut tokio::net::TcpStream) -> std::io::Result<()> {
    let mut buffer = [0_u8; 1024];
    let bytes_read = socket.read(&mut buffer).await?;
    if bytes_read == 0 {
        return Ok(());
    }
    Ok(())
}

async fn respond_with_metrics(
    handle: &PrometheusHandle,
    socket: &mut tokio::net::TcpStream,
) -> Result<()> {
    let body = handle.render();
    let body_bytes = body.as_bytes();
    let headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain; version=0.0.4\r\ncache-control: no-cache\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        body_bytes.len(),
    );
    socket
        .write_all(headers.as_bytes())
        .await
        .context("failed to stream metrics headers")?;
    socket
        .write_all(body_bytes)
        .await
        .context("failed to stream metrics payload")?;
    socket
        .shutdown()
        .await
        .context("failed to shutdown metrics connection")?;
    Ok(())
}
