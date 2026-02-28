use anyhow::Result;
use signalk_internal::{
    protocol::PathQueryResponse,
    server::{Callbacks, InternalState, serve_internal_api},
};
use signalk_server::{
    ServerState, build_router,
    config::ServerConfig,
    plugins::{
        delta_filter::DeltaFilterChain, host::PutHandlerRegistry, manager::PluginManager,
        routes::PluginRouteTable,
    },
};
use signalk_store::store::SignalKStore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

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
    info!(config_vessel_uuid = %config.vessel.uuid, "config loaded");
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;

    info!(vessel_uri = %config.vessel.uuid, "signalk-rs starting");

    let (store, _rx) = SignalKStore::new(&config.vessel.uuid);

    // ── Source priorities ─────────────────────────────────────────────────────
    if !config.source_priorities.is_empty() {
        store
            .blocking_write()
            .set_source_priorities(config.source_priorities.clone());
        info!(
            count = config.source_priorities.len(),
            "Source priorities configured"
        );
    }

    // ── Internal API (UDS) ────────────────────────────────────────────────────
    let bridge_token = if config.internal.bridge_token.is_empty() {
        let token = uuid::Uuid::new_v4().to_string();
        eprintln!("signalk-rs: SIGNALK_BRIDGE_TOKEN={token}");
        token
    } else {
        config.internal.bridge_token.clone()
    };

    // ── Shared maps: bridge registrations visible to the public API ──────────
    let put_handlers: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));
    let plugin_routes: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));

    // ── Shared plugin registry (used by admin API + on_bridge_plugins callback) ──
    let plugin_registry = Arc::new(RwLock::new(
        signalk_server::plugins::registry::PluginRegistry::new(),
    ));
    let registry_for_bridge = plugin_registry.clone();

    // ── Delta filter chain (shared between plugins and Internal API) ─────────
    let delta_filter = Arc::new(DeltaFilterChain::new());

    let store_for_delta = store.clone();
    let store_for_query = store.clone();
    let store_for_metadata = store.clone();
    let store_for_sources = store.clone();
    let filter_for_delta = delta_filter.clone();

    let internal_state = InternalState::new_shared(
        bridge_token,
        Callbacks {
            on_delta: Box::new(move |delta| {
                let s = store_for_delta.clone();
                let f = filter_for_delta.clone();
                tokio::spawn(async move {
                    if let Some(delta) = f.apply(delta) {
                        s.write().await.apply_delta(delta);
                    }
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
            on_metadata: Box::new(move |path| {
                let s = store_for_metadata.clone();
                Box::pin(async move { s.read().await.effective_metadata(&path) })
            }),
            on_source_query: Box::new(move |path| {
                let s = store_for_sources.clone();
                Box::pin(async move {
                    let store = s.read().await;
                    store.get_self_path_sources(&path).map(|sources| {
                        sources
                            .iter()
                            .map(|(src, sv)| (src.clone(), sv.value.clone()))
                            .collect()
                    })
                })
            }),
            on_bridge_plugins: Box::new(move |report| {
                let reg = registry_for_bridge.clone();
                tokio::spawn(async move {
                    let mut registry = reg.write().await;
                    for entry in &report.plugins {
                        registry.register_tier2(
                            signalk_server::plugins::registry::BridgePluginInfo {
                                id: entry.id.clone(),
                                name: entry.name.clone(),
                                version: entry.version.clone(),
                                description: entry.description.clone(),
                                has_webapp: entry.has_webapp,
                            },
                        );
                    }
                    tracing::info!(
                        count = report.plugins.len(),
                        "Bridge plugins registered in plugin registry"
                    );
                });
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

    // ── Plugin infrastructure ────────────────────────────────────────────────
    let route_table = Arc::new(PluginRouteTable::new());
    let put_handler_registry = Arc::new(PutHandlerRegistry::new());
    let webapp_registry = Arc::new(RwLock::new(signalk_server::webapps::WebappRegistry::new()));

    let config_dir = PathBuf::from(&config.data_dir).join("plugin-config");
    let data_dir = PathBuf::from(&config.data_dir).join("plugin-data");

    let mut plugin_manager = PluginManager::new(
        store.clone(),
        route_table.clone(),
        put_handler_registry.clone(),
        put_handlers.clone(),
        plugin_routes.clone(),
        delta_filter,
        webapp_registry.clone(),
        config_dir,
        data_dir,
    );

    // Register all compiled-in Tier 1 plugins
    plugin_manager.register(Box::new(nmea0183_receive::Nmea0183TcpPlugin::new()));
    plugin_manager.register(Box::new(nmea0183_receive::Nmea0183SerialPlugin::new()));
    plugin_manager.register(Box::new(anchor_alarm::AnchorAlarmPlugin::new()));
    plugin_manager.register(Box::new(derived_data::DerivedDataPlugin::new()));
    plugin_manager.register(Box::new(ais_status::AisStatusPlugin::new()));
    #[cfg(feature = "simulator")]
    plugin_manager.register(Box::new(sensor_data_simulator::SimulatorPlugin::new()));
    #[cfg(feature = "nmea0183-output")]
    plugin_manager.register(Box::new(nmea0183_send::Nmea0183SendPlugin::new()));
    #[cfg(feature = "nmea2000")]
    plugin_manager.register(Box::new(nmea2000_receive::Nmea2000Plugin::new()));
    #[cfg(feature = "nmea2000")]
    plugin_manager.register(Box::new(nmea2000_send::Nmea2000SendPlugin::new()));

    // Build plugin configs from [[plugins]] section
    let plugin_configs: HashMap<String, serde_json::Value> = config
        .plugins
        .iter()
        .filter(|pc| pc.enabled)
        .map(|pc| (pc.id.clone(), pc.config.clone()))
        .collect();

    // Start all plugins that have config entries
    plugin_manager.start_all(&plugin_configs).await;

    // Wrap PluginManager in Arc<Mutex> for shared access (admin API)
    let plugin_manager = Arc::new(tokio::sync::Mutex::new(plugin_manager));

    // ── Public HTTP + WebSocket server ────────────────────────────────────────
    let resource_providers = Arc::new(signalk_server::resources::ResourceProviderRegistry::new(
        Arc::new(signalk_server::resources::FileResourceProvider::new(
            PathBuf::from(&config.data_dir).join("resources"),
        )),
    ));

    let state = ServerState::new_shared(
        config.clone(),
        store,
        put_handlers,
        plugin_routes,
        put_handler_registry,
        route_table,
        plugin_manager.clone(),
        plugin_registry,
        webapp_registry.clone(),
        resource_providers,
    );

    // Populate plugin registry with initial Tier 1 statuses
    {
        let mgr = plugin_manager.lock().await;
        signalk_server::api::admin::populate_registry_from_manager(&state, &mgr).await;
    }

    // Discover webapps from node_modules + collect plugin-registered webapps
    let discovered =
        signalk_server::webapps::discover_webapps(std::path::Path::new(&config.modules_dir));
    {
        let mut registry = webapp_registry.write().await;
        for webapp in &discovered {
            registry.register(webapp.clone());
        }
    }
    let webapps = {
        let registry = webapp_registry.read().await;
        registry.all().to_vec()
    };
    if !webapps.is_empty() {
        info!(count = webapps.len(), "Discovered webapps");
    }
    let router = build_router(state, &webapps);

    info!(%addr, "Listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
