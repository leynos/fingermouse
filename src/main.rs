//! fingermouse entrypoint.
mod config;
mod framing;
mod identity;
mod query;
mod rate_limit;
mod server;
mod storage;
mod user;

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use config::{CliOptions, ServerConfig};
use rate_limit::RateLimiter;
use server::FingerServer;
use storage::ObjectStoreUserStore;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    install_tracing();

    let cli = CliOptions::parse();
    let config = ServerConfig::from_cli(cli)?;
    let store = config.build_store()?;

    let repository = ObjectStoreUserStore::new(
        Arc::clone(&store),
        config.profile_prefix.clone(),
        config.plan_prefix.clone(),
    );

    let config = Arc::new(config);
    let limiter = Arc::new(RateLimiter::new(config.rate.clone()));
    let repository = Arc::new(repository);

    let server = FingerServer::new(config, repository, limiter);
    server.run().await
}

fn install_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
