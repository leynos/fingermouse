//! Tokio-based finger server implementation.
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tracing::{info, warn};

use crate::config::ServerConfig;
use crate::framing::CrlfBuffer;
use crate::identity::HostName;
use crate::query::{self, FingerQuery};
use crate::rate_limit::{RateLimitError, RateLimiter};
use crate::storage::{RepositoryError, UserRecord, UserStore};

const GENERIC_ERROR: &str = "fingermouse: request refused";
const HOST_MISMATCH: &str = "fingermouse: host not served here";
const RATE_LIMIT_MESSAGE: &str = "fingermouse: slow down";
const USER_MISSING: &str = "fingermouse: no matching record";

// Centralise connection error logging to keep the spawn closure shallow for clippy.
fn log_connection_failure(remote: SocketAddr, outcome: Result<()>) {
    if let Err(err) = outcome {
        warn!(remote = %remote, error = %err, "connection handling failed");
    }
}

/// Tokio-based TCP server that answers finger protocol requests.
pub struct FingerServer<S>
where
    S: UserStore + 'static,
{
    config: Arc<ServerConfig>,
    store: Arc<S>,
    limiter: Arc<RateLimiter>,
}

impl<S> FingerServer<S>
where
    S: UserStore + 'static,
{
    /// Create a new server instance using the supplied components.
    pub const fn new(config: Arc<ServerConfig>, store: Arc<S>, limiter: Arc<RateLimiter>) -> Self {
        Self {
            config,
            store,
            limiter,
        }
    }

    /// Run the listener loop and spawn tasks for incoming connections.
    pub async fn run(self) -> Result<()> {
        let listener = TcpListener::bind(self.config.listen)
            .await
            .with_context(|| format!("failed to bind {}", self.config.listen))?;
        info!(address = %self.config.listen, "listening for finger requests");

        loop {
            let (socket, addr) = listener.accept().await?;
            let handler = ConnectionHandler {
                config: Arc::clone(&self.config),
                store: Arc::clone(&self.store),
                limiter: Arc::clone(&self.limiter),
            };
            tokio::spawn(async move {
                let outcome = handler.serve(socket, addr).await;
                log_connection_failure(addr, outcome);
            });
        }
    }
}

struct ConnectionHandler<S>
where
    S: UserStore + 'static,
{
    config: Arc<ServerConfig>,
    store: Arc<S>,
    limiter: Arc<RateLimiter>,
}

impl<S> ConnectionHandler<S>
where
    S: UserStore + 'static,
{
    async fn serve(&self, mut stream: TcpStream, addr: SocketAddr) -> Result<()> {
        let remote_ip = addr.ip();
        if self.enforce_rate_limit(&mut stream, remote_ip).await? {
            return Ok(());
        }

        let raw_request = self.read_query(&mut stream).await?;
        if let Some(query) = self
            .parse_query(&raw_request, remote_ip, &mut stream)
            .await?
        {
            self.respond_to_query(&mut stream, query).await?;
        }
        stream.shutdown().await.ok();

        Ok(())
    }

    async fn enforce_rate_limit(&self, stream: &mut TcpStream, remote_ip: IpAddr) -> Result<bool> {
        match self.limiter.check(remote_ip).await {
            Ok(()) => Ok(false),
            Err(RateLimitError::LimitExceeded { retry_in }) => {
                warn!(remote = %remote_ip, ?retry_in, "rate limit exceeded");
                self.write_response(stream, RATE_LIMIT_MESSAGE).await?;
                stream.shutdown().await.ok();
                Ok(true)
            }
        }
    }

    async fn read_query(&self, stream: &mut TcpStream) -> Result<Vec<u8>> {
        let mut reader = BufReader::new(stream);
        self.read_request(&mut reader).await
    }

    async fn parse_query(
        &self,
        raw_request: &[u8],
        remote_ip: IpAddr,
        stream: &mut TcpStream,
    ) -> Result<Option<FingerQuery>> {
        match query::parse(raw_request) {
            Ok(query) => Ok(Some(query)),
            Err(err) => {
                warn!(remote = %remote_ip, error = %err, "invalid query");
                self.write_response(stream, GENERIC_ERROR).await?;
                Ok(None)
            }
        }
    }

    async fn respond_to_query(&self, stream: &mut TcpStream, query: FingerQuery) -> Result<()> {
        let host = query
            .host
            .clone()
            .unwrap_or_else(|| self.config.default_host.clone());
        let payload = self.build_response(query, host).await;
        let write_result = timeout(self.config.request_timeout, stream.write_all(&payload))
            .await
            .context("response write timeout")?;
        write_result.context("sending response")?;
        Ok(())
    }

    async fn read_request<R>(&self, reader: &mut R) -> Result<Vec<u8>>
    where
        R: AsyncBufRead + Unpin,
    {
        let mut buffer = Vec::with_capacity(self.config.max_request_bytes);
        let limit = (self.config.max_request_bytes + 1) as u64;
        let mut limited = reader.take(limit);
        let amount = timeout(
            self.config.request_timeout,
            limited.read_until(b'\n', &mut buffer),
        )
        .await
        .context("request timeout")??;
        if amount == 0 {
            return Err(anyhow::anyhow!("connection closed"));
        }
        if buffer.len() > self.config.max_request_bytes {
            return Err(anyhow::anyhow!("request too large"));
        }
        Ok(buffer)
    }

    async fn build_response(&self, query: FingerQuery, host: HostName) -> Vec<u8> {
        let include_plan = query.verbose;
        if !self.config.accepts_host(&host) {
            return render_message(HOST_MISMATCH);
        }
        match self.store.load_user(&query.username, include_plan).await {
            Ok(UserRecord { profile, plan }) => {
                profile.render(include_plan, plan.as_deref()).as_bytes()
            }
            Err(RepositoryError::NotFound) => render_message(USER_MISSING),
            Err(RepositoryError::InvalidPlanEncoding) => {
                warn!(user = %query.username, "invalid plan encoding");
                render_message(GENERIC_ERROR)
            }
            Err(RepositoryError::Parse(err)) => {
                warn!(user = %query.username, error = %err, "profile parse error");
                render_message(GENERIC_ERROR)
            }
            Err(RepositoryError::Storage { source }) => {
                warn!(error = %source, "unexpected backend error");
                render_message(GENERIC_ERROR)
            }
        }
    }

    async fn write_response(&self, stream: &mut TcpStream, message: &str) -> Result<()> {
        let payload = render_message(message);
        let write_result = timeout(self.config.request_timeout, stream.write_all(&payload))
            .await
            .context("response write timeout")?;
        write_result.context("writing response")?;
        Ok(())
    }
}

fn render_message(message: &str) -> Vec<u8> {
    let mut buffer = CrlfBuffer::with_capacity(message.len() + 2);
    buffer.push_line(message);
    buffer.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{HostName, Username};
    use crate::rate_limit::RateLimitSettings;
    use crate::user::FingerProfile;
    use anyhow::{Result, anyhow};
    use futures::FutureExt;
    use futures::future::BoxFuture;
    use indexmap::IndexSet;
    use rstest::rstest;
    use std::path::PathBuf;
    use std::time::Duration;

    #[derive(Clone)]
    struct StaticStore {
        record: Option<UserRecord>,
    }

    impl UserStore for StaticStore {
        fn load_user<'a>(
            &'a self,
            _username: &'a Username,
            _include_plan: bool,
        ) -> BoxFuture<'a, Result<UserRecord, RepositoryError>> {
            async move { self.record.clone().ok_or(RepositoryError::NotFound) }.boxed()
        }
    }

    fn base_config() -> Result<Arc<ServerConfig>> {
        let default_host = HostName::parse("localhost").map_err(|err| anyhow!(err))?;
        let listen = "127.0.0.1:0"
            .parse()
            .map_err(|err| anyhow!("invalid listen address: {err}"))?;
        Ok(Arc::new(ServerConfig {
            listen,
            default_host: default_host.clone(),
            allowed_hosts: IndexSet::from([default_host]),
            store_root: PathBuf::from("."),
            profile_prefix: "profiles".to_owned(),
            plan_prefix: "plans".to_owned(),
            rate: RateLimitSettings {
                max_requests: 10,
                window: Duration::from_secs(60),
                max_entries: 8192,
            },
            metrics_listen: None,
            request_timeout: Duration::from_secs(5),
            max_request_bytes: 512,
        }))
    }

    fn build_profile(username: &str) -> Result<FingerProfile> {
        let user = Username::parse(username).map_err(|err| anyhow!(err))?;
        let payload = format!(
            r#"
                username = "{username}"
                full_name = "Test User"
            "#
        );
        FingerProfile::parse(user, payload.as_bytes()).map_err(|err| anyhow!(err))
    }

    fn rate_limiter() -> Arc<RateLimiter> {
        Arc::new(RateLimiter::new(RateLimitSettings {
            max_requests: 10,
            window: Duration::from_secs(1),
            max_entries: 8192,
        }))
    }

    #[derive(Clone, Copy)]
    struct ResponseCase {
        username: &'static str,
        verbose: bool,
        plan: Option<&'static str>,
        expected_content: &'static str,
        expected_plan: Option<&'static str>,
    }

    #[rstest]
    #[case::serves_user(ResponseCase {
        username: "alice",
        verbose: true,
        plan: Some("Plan line"),
        expected_content: "Test User",
        expected_plan: Some("Plan line"),
    })]
    #[case::rejects_unknown_user(ResponseCase {
        username: "bob",
        verbose: false,
        plan: None,
        expected_content: USER_MISSING,
        expected_plan: None,
    })]
    #[tokio::test]
    async fn build_response_scenarios(#[case] case: ResponseCase) -> Result<()> {
        let store = match case.username {
            "alice" => {
                let profile = build_profile("alice")?;
                let record = UserRecord {
                    profile,
                    plan: case.plan.map(str::to_string),
                };
                Arc::new(StaticStore {
                    record: Some(record),
                })
            }
            _ => Arc::new(StaticStore { record: None }),
        };

        let handler = ConnectionHandler {
            config: base_config()?,
            store,
            limiter: rate_limiter(),
        };

        let parsed_username = Username::parse(case.username).map_err(|err| anyhow!(err))?;
        let query = FingerQuery {
            username: parsed_username,
            host: None,
            verbose: case.verbose,
        };
        let response_bytes = handler
            .build_response(query, handler.config.default_host.clone())
            .await;
        let response = String::from_utf8(response_bytes)?;
        assert!(
            response.contains(case.expected_content),
            "expected response to include '{}', got '{response}'",
            case.expected_content
        );
        if let Some(plan_fragment) = case.expected_plan {
            assert!(
                response.contains(plan_fragment),
                "expected response to include plan snippet '{plan_fragment}'"
            );
        }
        Ok(())
    }
}
