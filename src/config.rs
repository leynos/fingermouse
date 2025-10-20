//! Application configuration and command-line parsing.
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use object_store::ObjectStore;
use object_store::local::LocalFileSystem;
use std::sync::Arc;

use crate::identity::{HostName, IdentityError};
use crate::rate_limit::RateLimitSettings;

#[derive(Debug, Parser)]
#[command(name = "fingermouse")]
pub struct CliOptions {
    #[arg(long, env = "FINGERMOUSE_LISTEN", default_value = "0.0.0.0:7979")]
    pub listen: SocketAddr,
    #[arg(long, env = "FINGERMOUSE_DEFAULT_HOST", default_value = "localhost")]
    pub default_host: String,
    #[arg(long, env = "FINGERMOUSE_ALLOWED_HOSTS")]
    pub allowed_hosts: Option<String>,
    #[arg(long, env = "FINGERMOUSE_STORE_ROOT", default_value = "./data")]
    pub store_root: PathBuf,
    #[arg(long, env = "FINGERMOUSE_PROFILE_PREFIX", default_value = "profiles")]
    pub profile_prefix: String,
    #[arg(long, env = "FINGERMOUSE_PLAN_PREFIX", default_value = "plans")]
    pub plan_prefix: String,
    #[arg(long, env = "FINGERMOUSE_RATE_LIMIT", default_value_t = 30)]
    pub rate_limit: u32,
    #[arg(long, env = "FINGERMOUSE_RATE_WINDOW_SECS", default_value_t = 60)]
    pub rate_window_secs: u64,
    #[arg(long, env = "FINGERMOUSE_REQUEST_TIMEOUT_MS", default_value_t = 3000)]
    pub request_timeout_ms: u64,
    #[arg(long, env = "FINGERMOUSE_MAX_REQUEST_BYTES", default_value_t = 512)]
    pub max_request_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub default_host: HostName,
    pub allowed_hosts: Vec<HostName>,
    pub store_root: PathBuf,
    pub profile_prefix: String,
    pub plan_prefix: String,
    pub rate: RateLimitSettings,
    pub request_timeout: Duration,
    pub max_request_bytes: usize,
}

impl ServerConfig {
    pub fn from_cli(cli: CliOptions) -> Result<Self> {
        let default_host = HostName::parse(&cli.default_host)
            .map_err(|err| anyhow!("invalid default host: {}", describe_identity(&err)))?;

        let mut allowed_hosts = vec![default_host.clone()];
        if let Some(list) = cli.allowed_hosts {
            for item in list.split(',') {
                let trimmed = item.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let host = HostName::parse(trimmed).map_err(|err| {
                    anyhow!("invalid host in allow list: {}", describe_identity(&err))
                })?;
                if !allowed_hosts.contains(&host) {
                    allowed_hosts.push(host);
                }
            }
        }

        let rate = RateLimitSettings {
            max_requests: cli.rate_limit.max(1),
            window: Duration::from_secs(cli.rate_window_secs.max(1)),
        };

        Ok(Self {
            listen: cli.listen,
            default_host,
            allowed_hosts,
            store_root: cli.store_root,
            profile_prefix: sanitise_prefix(&cli.profile_prefix),
            plan_prefix: sanitise_prefix(&cli.plan_prefix),
            rate,
            request_timeout: Duration::from_millis(cli.request_timeout_ms.max(1)),
            max_request_bytes: cli.max_request_bytes.clamp(64, 4096),
        })
    }

    pub fn build_store(&self) -> Result<Arc<dyn ObjectStore>> {
        ensure_directory(&self.store_root)?;
        let store = LocalFileSystem::new_with_prefix(&self.store_root)
            .context("failed to initialise object store")?;
        let store: Arc<dyn ObjectStore> = Arc::new(store);
        Ok(store)
    }

    pub fn accepts_host(&self, host: &HostName) -> bool {
        self.allowed_hosts.contains(host)
    }
}

fn ensure_directory(path: &Path) -> Result<()> {
    if path.exists() {
        if path.is_dir() {
            return Ok(());
        }
        return Err(anyhow!(
            "store root must be a directory: {}",
            path.display()
        ));
    }
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create store root {}", path.display()))?;
    Ok(())
}

fn sanitise_prefix(input: &str) -> String {
    input.trim_matches('/').to_string()
}

fn describe_identity(err: &IdentityError) -> String {
    err.to_string()
}
