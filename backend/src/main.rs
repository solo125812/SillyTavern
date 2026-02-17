//! SillyTavern Rust sidecar backend.
//!
//! This binary loads the sidecar configuration, builds the Axum router,
//! and starts the HTTP server that the Node front door proxies to.

use std::net::SocketAddr;

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use sillytavern_backend::api::{router::build_router, settings::backup_settings_for_all_users};
use sillytavern_backend::config::AppConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing / logging
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load configuration
    let config = AppConfig::load()?;

    tracing::info!(
        data_root = %config.data_root.display(),
        "Loaded sidecar configuration"
    );

    // Backup settings for all users (mirrors Node startup behavior)
    backup_settings_for_all_users(&config.data_root);

    // Build the Axum router with shared application state
    let app = build_router(config.clone());

    // Determine listen address
    let addr: SocketAddr = config
        .listen_address
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 5050)));

    tracing::info!(%addr, "Starting SillyTavern Rust sidecar");

    // Start the server
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;

    Ok(())
}
