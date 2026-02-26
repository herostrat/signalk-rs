use anyhow::Result;
use signalk_server::{ServerState, build_router, config::ServerConfig};
use signalk_store::store::SignalKStore;
use std::net::SocketAddr;
use tracing::{info, warn};

/// Load configuration from a TOML file, falling back to defaults.
///
/// File path is taken from `SIGNALK_CONFIG_FILE` env var, then the standard
/// system path `/etc/signalk-rs/config.toml`, then in-tree `signalk-rs.toml`.
/// Missing files are silently ignored — useful for development without a config.
fn load_config() -> ServerConfig {
    let paths: &[&str] = &[
        // Explicit override (e.g. Docker container)
        &std::env::var("SIGNALK_CONFIG_FILE").unwrap_or_default(),
        "/etc/signalk-rs/config.toml",
        "signalk-rs.toml",
    ];

    for path in paths {
        if path.is_empty() || !std::path::Path::new(path).exists() {
            continue;
        }
        let result = config::Config::builder()
            .add_source(config::File::from(std::path::Path::new(path)))
            .build()
            .and_then(|c| c.try_deserialize::<ServerConfig>());

        match result {
            Ok(cfg) => {
                // tracing not yet initialised here, use eprintln
                eprintln!("signalk-rs: loaded config from {path}");
                return cfg;
            }
            Err(e) => {
                eprintln!("signalk-rs: could not parse {path}: {e} — using defaults");
            }
        }
    }

    ServerConfig::default()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("signalk_server=debug".parse()?)
                .add_directive("signalk_store=debug".parse()?),
        )
        .init();

    let config = load_config();
    warn!(config_vessel_uuid = %config.vessel.uuid, "config loaded");
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;

    info!(vessel_uri = %config.vessel.uuid, "signalk-rs starting");

    let (store, _rx) = SignalKStore::new(&config.vessel.uuid);
    let state = ServerState::new(config.clone(), store);
    let router = build_router(state);

    info!(%addr, "Listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
