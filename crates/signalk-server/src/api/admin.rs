/// Admin API handlers — plugin management and server administration.
///
/// Routes:
///   GET    /admin/api/plugins                    → list all plugins (unified registry)
///   GET    /admin/api/plugins/{pluginId}         → get single plugin info
///   GET    /admin/api/plugins/{pluginId}/config  → get plugin config schema + current config
///   PUT    /admin/api/plugins/{pluginId}/config  → update plugin config (restart if running)
///   POST   /admin/api/plugins/{pluginId}/restart → restart a running plugin
///   POST   /admin/api/plugins/{pluginId}/enable  → start a stopped plugin
///   POST   /admin/api/plugins/{pluginId}/disable → stop a running plugin
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use tracing::info;

use crate::ServerState;
use crate::plugins::registry::PluginTier;

/// GET /admin/api/plugins — list all plugins from the unified registry.
pub async fn list_plugins(State(state): State<Arc<ServerState>>) -> Response {
    let registry = state.plugin_registry.read().await;
    Json(registry.all()).into_response()
}

/// GET /admin/api/plugins/{plugin_id} — get a single plugin's info.
pub async fn get_plugin(
    State(state): State<Arc<ServerState>>,
    Path(plugin_id): Path<String>,
) -> Response {
    let registry = state.plugin_registry.read().await;
    match registry.get(&plugin_id) {
        Some(info) => Json(info.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"message": format!("Plugin not found: {plugin_id}")})),
        )
            .into_response(),
    }
}

/// GET /admin/api/plugins/{plugin_id}/config — get plugin's config schema and current options.
pub async fn get_plugin_config(
    State(state): State<Arc<ServerState>>,
    Path(plugin_id): Path<String>,
) -> Response {
    let registry = state.plugin_registry.read().await;
    let info = match registry.get(&plugin_id) {
        Some(info) => info.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"message": format!("Plugin not found: {plugin_id}")})),
            )
                .into_response();
        }
    };
    drop(registry);

    // Only Tier 1 plugins have configs we can read directly
    let current_config = if info.tier == PluginTier::Rust {
        let mgr = state.plugin_manager.lock().await;
        mgr.read_plugin_config(&plugin_id)
            .unwrap_or(serde_json::Value::Null)
    } else {
        serde_json::Value::Null
    };

    Json(serde_json::json!({
        "schema": info.schema,
        "config": current_config,
    }))
    .into_response()
}

/// PUT /admin/api/plugins/{plugin_id}/config — update config and optionally restart.
pub async fn update_plugin_config(
    State(state): State<Arc<ServerState>>,
    Path(plugin_id): Path<String>,
    Json(new_config): Json<serde_json::Value>,
) -> Response {
    let registry = state.plugin_registry.read().await;
    let info = match registry.get(&plugin_id) {
        Some(info) => info.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"message": format!("Plugin not found: {plugin_id}")})),
            )
                .into_response();
        }
    };
    drop(registry);

    if info.tier != PluginTier::Rust {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": "Config update only supported for Tier 1 (Rust) plugins"})),
        )
            .into_response();
    }

    let mut mgr = state.plugin_manager.lock().await;

    // Save config
    mgr.save_plugin_config(&plugin_id, &new_config);

    // If running, restart with new config
    let was_running = mgr.is_running(&plugin_id);
    if was_running {
        if let Err(e) = mgr.stop_plugin(&plugin_id).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": format!("Failed to stop plugin: {e}")})),
            )
                .into_response();
        }
        if let Err(e) = mgr.start_plugin(&plugin_id, new_config).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": format!("Failed to restart plugin: {e}")})),
            )
                .into_response();
        }
    }

    // Update registry
    sync_tier1_status(&state, &mgr).await;

    info!(plugin = %plugin_id, restarted = was_running, "Plugin config updated");
    StatusCode::NO_CONTENT.into_response()
}

/// POST /admin/api/plugins/{plugin_id}/restart — restart a running plugin.
pub async fn restart_plugin(
    State(state): State<Arc<ServerState>>,
    Path(plugin_id): Path<String>,
) -> Response {
    let registry = state.plugin_registry.read().await;
    let info = match registry.get(&plugin_id) {
        Some(info) => info.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"message": format!("Plugin not found: {plugin_id}")})),
            )
                .into_response();
        }
    };
    drop(registry);

    match info.tier {
        PluginTier::Rust => {
            let mut mgr = state.plugin_manager.lock().await;
            let config = mgr
                .read_plugin_config(&plugin_id)
                .unwrap_or(serde_json::json!({}));
            if let Err(e) = mgr.stop_plugin(&plugin_id).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"message": format!("Failed to stop plugin: {e}")})),
                )
                    .into_response();
            }
            if let Err(e) = mgr.start_plugin(&plugin_id, config).await {
                sync_tier1_status(&state, &mgr).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"message": format!("Failed to start plugin: {e}")})),
                )
                    .into_response();
            }
            sync_tier1_status(&state, &mgr).await;
        }
        PluginTier::Bridge => {
            if let Err(e) = bridge_lifecycle(&state, &plugin_id, "restart").await {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"message": format!("Bridge lifecycle failed: {e}")})),
                )
                    .into_response();
            }
        }
        PluginTier::Standalone => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"message": "Cannot control standalone plugins"})),
            )
                .into_response();
        }
    }

    info!(plugin = %plugin_id, tier = ?info.tier, "Plugin restarted");
    StatusCode::NO_CONTENT.into_response()
}

/// POST /admin/api/plugins/{plugin_id}/enable — start a stopped plugin.
pub async fn enable_plugin(
    State(state): State<Arc<ServerState>>,
    Path(plugin_id): Path<String>,
) -> Response {
    let registry = state.plugin_registry.read().await;
    let info = match registry.get(&plugin_id) {
        Some(info) => info.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"message": format!("Plugin not found: {plugin_id}")})),
            )
                .into_response();
        }
    };
    drop(registry);

    match info.tier {
        PluginTier::Rust => {
            let mut mgr = state.plugin_manager.lock().await;
            let config = mgr
                .read_plugin_config(&plugin_id)
                .unwrap_or(serde_json::json!({}));
            if let Err(e) = mgr.start_plugin(&plugin_id, config).await {
                sync_tier1_status(&state, &mgr).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"message": format!("Failed to start plugin: {e}")})),
                )
                    .into_response();
            }
            sync_tier1_status(&state, &mgr).await;
        }
        PluginTier::Bridge => {
            if let Err(e) = bridge_lifecycle(&state, &plugin_id, "start").await {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"message": format!("Bridge lifecycle failed: {e}")})),
                )
                    .into_response();
            }
            state
                .plugin_registry
                .write()
                .await
                .update_status(&plugin_id, "running");
        }
        PluginTier::Standalone => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"message": "Cannot control standalone plugins"})),
            )
                .into_response();
        }
    }

    info!(plugin = %plugin_id, tier = ?info.tier, "Plugin enabled");
    StatusCode::NO_CONTENT.into_response()
}

/// POST /admin/api/plugins/{plugin_id}/disable — stop a running plugin.
pub async fn disable_plugin(
    State(state): State<Arc<ServerState>>,
    Path(plugin_id): Path<String>,
) -> Response {
    let registry = state.plugin_registry.read().await;
    let info = match registry.get(&plugin_id) {
        Some(info) => info.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"message": format!("Plugin not found: {plugin_id}")})),
            )
                .into_response();
        }
    };
    drop(registry);

    match info.tier {
        PluginTier::Rust => {
            let mut mgr = state.plugin_manager.lock().await;
            if let Err(e) = mgr.stop_plugin(&plugin_id).await {
                sync_tier1_status(&state, &mgr).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"message": format!("Failed to stop plugin: {e}")})),
                )
                    .into_response();
            }
            sync_tier1_status(&state, &mgr).await;
        }
        PluginTier::Bridge => {
            if let Err(e) = bridge_lifecycle(&state, &plugin_id, "stop").await {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"message": format!("Bridge lifecycle failed: {e}")})),
                )
                    .into_response();
            }
            state
                .plugin_registry
                .write()
                .await
                .update_status(&plugin_id, "stopped");
        }
        PluginTier::Standalone => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"message": "Cannot control standalone plugins"})),
            )
                .into_response();
        }
    }

    info!(plugin = %plugin_id, tier = ?info.tier, "Plugin disabled");
    StatusCode::NO_CONTENT.into_response()
}

/// Send a lifecycle command to the bridge for a Tier 2 plugin.
async fn bridge_lifecycle(
    state: &ServerState,
    plugin_id: &str,
    action: &str,
) -> anyhow::Result<()> {
    let bridge_socket = std::path::PathBuf::from(&state.config.internal.uds_bridge_socket);
    let body = serde_json::json!({
        "event": action,
        "pluginId": plugin_id,
    });
    signalk_internal::uds::uds_post(&bridge_socket, "/lifecycle", &body).await?;
    Ok(())
}

/// Sync Tier 1 plugin statuses from PluginManager into PluginRegistry.
async fn sync_tier1_status(state: &ServerState, mgr: &crate::plugins::manager::PluginManager) {
    let mut registry = state.plugin_registry.write().await;
    for (meta, status) in mgr.statuses() {
        let (status_str, enabled) = match &status {
            signalk_plugin_api::PluginStatus::Stopped => ("stopped".to_string(), false),
            signalk_plugin_api::PluginStatus::Starting => ("starting".to_string(), true),
            signalk_plugin_api::PluginStatus::Running(msg) => (format!("running: {msg}"), true),
            signalk_plugin_api::PluginStatus::Stopping => ("stopping".to_string(), true),
            signalk_plugin_api::PluginStatus::Error(msg) => (format!("error: {msg}"), false),
        };
        registry.register_tier1(
            &meta.id,
            &meta.name,
            &meta.description,
            &meta.version,
            &status_str,
            enabled,
        );
    }
}

/// Populate PluginRegistry with initial Tier 1 statuses after start_all().
/// Also captures config schemas from each plugin.
pub async fn populate_registry_from_manager(
    state: &ServerState,
    mgr: &crate::plugins::manager::PluginManager,
) {
    sync_tier1_status(state, mgr).await;

    // Also set schemas from plugin metadata
    let mut registry = state.plugin_registry.write().await;
    for (meta, _) in mgr.statuses_with_schema() {
        if let Some(schema) = meta.schema
            && let Some(info) = registry.get_mut(&meta.id)
        {
            info.schema = Some(schema);
        }
    }
}
