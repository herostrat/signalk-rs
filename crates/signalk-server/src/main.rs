use anyhow::Result;
use signalk_internal::{
    protocol::PathQueryResponse,
    server::{Callbacks, InternalState, serve_internal_api},
};
use signalk_nmea0183::provider::NmeaTcpProvider;
use signalk_server::{ServerState, build_router, config::{InputConfig, ServerConfig}};
use signalk_store::store::SignalKStore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
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
                .add_directive("signalk_store=debug".parse()?)
                .add_directive("signalk_internal=debug".parse()?),
        )
        .init();

    let config = load_config();
    warn!(config_vessel_uuid = %config.vessel.uuid, "config loaded");
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;

    info!(vessel_uri = %config.vessel.uuid, "signalk-rs starting");

    let (store, _rx) = SignalKStore::new(&config.vessel.uuid);

    // ── Internal API (UDS) ────────────────────────────────────────────────────
    let bridge_token = if config.internal.bridge_token.is_empty() {
        let token = uuid::Uuid::new_v4().to_string();
        eprintln!("signalk-rs: SIGNALK_BRIDGE_TOKEN={token}");
        token
    } else {
        config.internal.bridge_token.clone()
    };

    // ── Shared maps: bridge registrations visible to the public API ──────────
    let put_handlers: Arc<RwLock<HashMap<String, String>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let plugin_routes: Arc<RwLock<HashMap<String, String>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let store_for_delta = store.clone();
    let store_for_query = store.clone();

    let internal_state = InternalState::new_shared(
        bridge_token,
        Callbacks {
            on_delta: Box::new(move |delta| {
                let s = store_for_delta.clone();
                tokio::spawn(async move {
                    s.write().await.apply_delta(delta);
                });
            }),
            on_query: Box::new(move |query| {
                let s = store_for_query.clone();
                Box::pin(async move {
                    let store = s.read().await;
                    let sk_value = store.get_self_path(&query.path)?;
                    Some(PathQueryResponse {
                        path: query.path.clone(),
                        value: Some(sk_value.value.clone()),
                        source: Some(sk_value.source.to_string()),
                        timestamp: Some(sk_value.timestamp.to_rfc3339()),
                    })
                })
            }),
        },
        put_handlers.clone(),
        plugin_routes.clone(),
    );

    let socket_path: std::path::PathBuf = config.internal.uds_rs_socket.clone().into();
    tokio::spawn(async move {
        if let Err(e) = serve_internal_api(socket_path, internal_state).await {
            tracing::error!("Internal API server error: {e}");
        }
    });

    // ── Input providers (NMEA 0183, etc.) ────────────────────────────────────
    // Deltas from all providers are fed directly into the store.
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<signalk_types::Delta>(256);

    for input in &config.inputs {
        match input {
            InputConfig::Nmea0183Tcp { addr, source_label } => {
                let addr: SocketAddr = addr.parse()?;
                let provider = NmeaTcpProvider::new(addr, source_label.clone());
                let tx = input_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = provider.run(tx).await {
                        tracing::error!("NMEA TCP provider error: {e}");
                    }
                });
            }
        }
    }

    // Fan-in: apply incoming deltas to the store.
    let store_for_input = store.clone();
    tokio::spawn(async move {
        while let Some(delta) = input_rx.recv().await {
            store_for_input.write().await.apply_delta(delta);
        }
    });
    // Drop the original sender so the fan-in task exits when all providers drop their clones.
    drop(input_tx);

    // ── Public HTTP + WebSocket server ────────────────────────────────────────
    let state = ServerState::new_shared(config.clone(), store, put_handlers, plugin_routes);
    let router = build_router(state);

    info!(%addr, "Listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
