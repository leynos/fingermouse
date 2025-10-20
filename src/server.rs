//! Tokio-based finger server implementation.
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tracing::{info, warn};

use crate::config::ServerConfig;
use crate::identity::HostName;
use crate::query::{self, FingerQuery};
use crate::rate_limit::{RateLimitError, RateLimiter};
use crate::storage::{RepositoryError, UserRecord, UserStore};

const GENERIC_ERROR: &str = "fingermouse: request refused";
const HOST_MISMATCH: &str = "fingermouse: host not served here";
const RATE_LIMIT_MESSAGE: &str = "fingermouse: slow down";
const USER_MISSING: &str = "fingermouse: no matching record";

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
    pub fn new(config: Arc<ServerConfig>, store: Arc<S>, limiter: Arc<RateLimiter>) -> Self {
        Self {
            config,
            store,
            limiter,
        }
    }

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
                if let Err(err) = handler.serve(socket, addr).await {
                    warn!(remote = %addr, error = %err, "connection handling failed");
                }
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
    async fn serve(&self, stream: TcpStream, addr: SocketAddr) -> Result<()> {
        let remote_ip = addr.ip();
        let mut stream = stream;
        if let Err(RateLimitError::LimitExceeded { retry_in }) = self.limiter.check(remote_ip).await
        {
            warn!(remote = %remote_ip, ?retry_in, "rate limit exceeded");
            self.write_response(&mut stream, RATE_LIMIT_MESSAGE).await?;
            stream.shutdown().await.ok();
            return Ok(());
        }

        let mut reader = BufReader::new(&mut stream);
        let raw_request = self.read_request(&mut reader).await?;
        drop(reader);
        match query::parse(&raw_request) {
            Ok(query) => {
                let host = query
                    .host
                    .clone()
                    .unwrap_or_else(|| self.config.default_host.clone());
                let payload = self.build_response(query, host).await;
                stream
                    .write_all(&payload)
                    .await
                    .context("sending response")?;
                stream.shutdown().await.ok();
            }
            Err(err) => {
                warn!(remote = %remote_ip, error = %err, "invalid query");
                self.write_response(&mut stream, GENERIC_ERROR).await?;
            }
        }

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
                warn!("invalid plan encoding for {}", query.username);
                render_message(GENERIC_ERROR)
            }
            Err(RepositoryError::Parse(err)) => {
                warn!("profile parse error for {}: {err}", query.username);
                render_message(GENERIC_ERROR)
            }
            Err(RepositoryError::Other(err)) => {
                warn!("unexpected backend error: {err}");
                render_message(GENERIC_ERROR)
            }
        }
    }

    async fn write_response(&self, stream: &mut TcpStream, message: &str) -> Result<()> {
        stream
            .write_all(&render_message(message))
            .await
            .context("writing response")?;
        Ok(())
    }
}

fn render_message(message: &str) -> Vec<u8> {
    let mut line = message.to_string();
    if !line.ends_with("\r\n") {
        line.push_str("\r\n");
    }
    line.into_bytes()
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
            allowed_hosts: vec![default_host],
            store_root: PathBuf::from("."),
            profile_prefix: "profiles".to_string(),
            plan_prefix: "plans".to_string(),
            rate: RateLimitSettings {
                max_requests: 10,
                window: Duration::from_secs(60),
            },
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
        }))
    }

    #[tokio::test]
    async fn serves_user_record() -> Result<()> {
        let profile = build_profile("alice")?;
        let record = UserRecord {
            profile,
            plan: Some("Plan line".to_string()),
        };
        let handler = ConnectionHandler {
            config: base_config()?,
            store: Arc::new(StaticStore {
                record: Some(record),
            }),
            limiter: rate_limiter(),
        };
        let username = Username::parse("alice").map_err(|err| anyhow!(err))?;
        let query = FingerQuery {
            username,
            host: None,
            verbose: true,
        };
        let response = handler
            .build_response(query, handler.config.default_host.clone())
            .await;
        let response = String::from_utf8(response)?;
        assert!(response.contains("Test User"));
        assert!(response.contains("Plan line"));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unknown_user() -> Result<()> {
        let handler = ConnectionHandler {
            config: base_config()?,
            store: Arc::new(StaticStore { record: None }),
            limiter: rate_limiter(),
        };
        let username = Username::parse("bob").map_err(|err| anyhow!(err))?;
        let query = FingerQuery {
            username,
            host: None,
            verbose: false,
        };
        let response = handler
            .build_response(query, handler.config.default_host.clone())
            .await;
        let response = String::from_utf8(response)?;
        assert!(response.contains(USER_MISSING));
        Ok(())
    }
}
