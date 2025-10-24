//! Application configuration and command-line parsing.
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use indexmap::IndexSet;
use object_store::{ObjectStore, local::LocalFileSystem};
use tracing::warn;

use crate::identity::{HostName, IdentityError};
use crate::rate_limit::RateLimitSettings;

#[derive(Debug, Parser)]
#[command(name = "fingermouse")]
/// Command-line options accepted by the fingermouse binary.
pub struct CliOptions {
    /// Address the server binds to.
    #[arg(long, env = "FINGERMOUSE_LISTEN", default_value = "0.0.0.0:7979")]
    pub listen: SocketAddr,
    /// Default hostname used when a client query omits one.
    #[arg(long, env = "FINGERMOUSE_DEFAULT_HOST", default_value = "localhost")]
    pub default_host: String,
    /// Optional comma-separated list of hostnames the server will answer.
    #[arg(long, env = "FINGERMOUSE_ALLOWED_HOSTS")]
    pub allowed_hosts: Option<String>,
    /// Root directory used by the object store backend.
    #[arg(long, env = "FINGERMOUSE_STORE_ROOT", default_value = "./data")]
    pub store_root: PathBuf,
    /// Relative path within the store containing profile TOML files.
    #[arg(long, env = "FINGERMOUSE_PROFILE_PREFIX", default_value = "profiles")]
    pub profile_prefix: String,
    /// Relative path within the store containing plan files.
    #[arg(long, env = "FINGERMOUSE_PLAN_PREFIX", default_value = "plans")]
    pub plan_prefix: String,
    /// Maximum number of requests allowed per window for a single client.
    #[arg(long, env = "FINGERMOUSE_RATE_LIMIT", default_value_t = 30)]
    pub rate_limit: u32,
    /// Length of the rate-limiting window in seconds.
    #[arg(long, env = "FINGERMOUSE_RATE_WINDOW_SECS", default_value_t = 60)]
    pub rate_window_secs: u64,
    /// Maximum number of client entries retained in the rate limiter.
    #[arg(long, env = "FINGERMOUSE_RATE_CAPACITY", default_value_t = 8192)]
    pub rate_capacity: usize,
    /// Socket address that serves Prometheus metrics; disabled when omitted.
    #[arg(long, env = "FINGERMOUSE_METRICS_LISTEN")]
    pub metrics_listen: Option<SocketAddr>,
    /// Timeout in milliseconds for reading the client query line.
    #[arg(long, env = "FINGERMOUSE_REQUEST_TIMEOUT_MS", default_value_t = 3000)]
    pub request_timeout_ms: u64,
    /// Maximum size of the accepted query line in bytes.
    #[arg(long, env = "FINGERMOUSE_MAX_REQUEST_BYTES", default_value_t = 512)]
    pub max_request_bytes: usize,
}

#[derive(Debug, Clone)]
/// Runtime configuration compiled from CLI arguments and environment.
pub struct ServerConfig {
    /// Address the TCP listener binds to.
    pub listen: SocketAddr,
    /// Default hostname used when queries omit one.
    pub default_host: HostName,
    /// Hostnames that the server will respond to.
    pub allowed_hosts: IndexSet<HostName>,
    /// Root directory used for the object store backend.
    pub store_root: PathBuf,
    /// Subdirectory containing profile TOML files.
    pub profile_prefix: String,
    /// Subdirectory containing plan files.
    pub plan_prefix: String,
    /// Rate limiting settings applied per client IP.
    pub rate: RateLimitSettings,
    /// Address that exposes Prometheus metrics, if configured.
    pub metrics_listen: Option<SocketAddr>,
    /// Maximum time spent waiting for a client query.
    pub request_timeout: Duration,
    /// Maximum query size accepted from a client.
    pub max_request_bytes: usize,
}

impl ServerConfig {
    /// Construct configuration from parsed command-line options.
    pub fn from_cli(cli: CliOptions) -> Result<Self> {
        let default_host = HostName::parse(&cli.default_host)
            .map_err(|err| anyhow!("invalid default host: {}", describe_identity(&err)))?;

        let mut allowed_hosts: IndexSet<HostName> = IndexSet::from([default_host.clone()]);
        if let Some(list) = cli.allowed_hosts {
            extend_allowed_hosts(&list, &mut allowed_hosts)?;
        }

        let rate = RateLimitSettings {
            max_requests: cli.rate_limit.max(1),
            window: Duration::from_secs(cli.rate_window_secs.max(1)),
            max_entries: cli.rate_capacity.max(1),
        };

        let max_request_bytes = {
            const MIN: usize = 64;
            const MAX: usize = 4096;
            let original = cli.max_request_bytes;
            let clamped = original.clamp(MIN, MAX);
            if original != clamped {
                warn!(
                    original = original,
                    allowed_min = MIN,
                    allowed_max = MAX,
                    clamped = clamped,
                    "max_request_bytes outside allowed range; clamped",
                );
            }
            clamped
        };

        Ok(Self {
            listen: cli.listen,
            default_host,
            allowed_hosts,
            store_root: cli.store_root,
            profile_prefix: sanitise_prefix(&cli.profile_prefix),
            plan_prefix: sanitise_prefix(&cli.plan_prefix),
            rate,
            metrics_listen: cli.metrics_listen,
            request_timeout: Duration::from_millis(cli.request_timeout_ms.max(1)),
            max_request_bytes,
        })
    }

    /// Create an object store instance rooted at the configured directory.
    pub fn build_store(&self) -> Result<Arc<dyn ObjectStore>> {
        ensure_directory(&self.store_root)?;
        let fs_store = LocalFileSystem::new_with_prefix(&self.store_root)
            .context("failed to initialise object store")?;
        let store: Arc<dyn ObjectStore> = Arc::new(fs_store);
        Ok(store)
    }

    /// Determine whether the server is responsible for the supplied host.
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
    input.trim_matches('/').to_owned()
}

fn extend_allowed_hosts(list: &str, allowed_hosts: &mut IndexSet<HostName>) -> Result<()> {
    // Preserve caller supplied ordering while deduplicating entries so the default
    // host remains the first entry.
    for candidate in list
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let host = HostName::parse(candidate)
            .map_err(|err| anyhow!("invalid host in allow list: {}", describe_identity(&err)))?;
        allowed_hosts.insert(host);
    }
    Ok(())
}

fn describe_identity(err: &IdentityError) -> String {
    err.to_string()
}
