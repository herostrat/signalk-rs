/// `/skServer/*` compatibility routes for the SignalK admin UI.
///
/// The admin UI sets `window.serverRoutesPrefix = '/skServer'` and calls all
/// API endpoints under this prefix. These handlers bridge to our existing
/// admin API or return static/stub responses.
///
/// **Thin API layer** — handlers only extract state, call data-layer methods,
/// and serialize the result. No business logic lives here.
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use std::collections::HashMap;
use std::sync::Arc;

use crate::ServerState;

/// `GET /skServer/loginStatus` — security status (no auth system yet).
pub async fn login_status() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "notLoggedIn",
        "readOnly": false,
        "allowNewUserRegistration": true,
        "allowDeviceAccessRequests": true,
        "authenticationRequired": false,
    }))
}

/// `GET /skServer/webapps` — list webapps (same as /signalk/v1/webapps).
pub async fn list_webapps(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let registry = state.webapp_registry.read().await;
    Json(registry.all().to_vec())
}

/// `GET /skServer/settings` — server settings.
pub async fn get_settings(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    Json(serde_json::json!({
        "interfaces": {},
        "options": {
            "mdns": false,
            "wsCompression": false,
            "enablePluginLogging": true,
            "accessLogging": false,
            "trustProxy": false,
        },
        "port": state.config.server.port,
        "sslport": 0,
        "loggingDirectory": "",
        "pruneContextsMinutes": 60,
        "keepMostRecentLogsOnly": false,
        "logCountToKeep": 24,
        "runFromSystemd": false,
        "courseApi": {
            "apiOnly": false,
        },
    }))
}

/// `GET /skServer/vessel` — vessel configuration (reads from config store).
pub async fn get_vessel(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    Json(serde_json::json!({
        "name": state.config_store.vessel_name(),
        "uuid": state.config_store.vessel_uuid(),
        "mmsi": state.config_store.vessel_mmsi(),
    }))
}

/// `PUT /skServer/vessel` — update vessel configuration and persist.
pub async fn put_vessel(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = body.get("name").and_then(|v| v.as_str()).map(String::from);
    let mmsi = body.get("mmsi").and_then(|v| v.as_str()).map(String::from);

    state.config_store.set_vessel(name.clone(), mmsi.clone());

    // Also update the in-memory store so /signalk/v1/api reflects changes
    let uuid = state.config_store.vessel_uuid();
    let mut store = state.store.write().await;
    store.set_vessel_identity(&uuid, name, mmsi);
    drop(store);

    StatusCode::NO_CONTENT.into_response()
}

/// `GET /skServer/providers` — data provider list from the plugin registry.
pub async fn list_providers(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let registry = state.plugin_registry.read().await;
    Json(registry.providers())
}

/// `GET /skServer/availablePaths` — all data paths in the self vessel's store.
pub async fn list_available_paths(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let store = state.store.read().await;
    Json(store.self_paths())
}

/// `GET /skServer/sourcePriorities` — configured source priority map.
pub async fn get_source_priorities(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    Json(state.config_store.source_priorities())
}

/// `PUT /skServer/sourcePriorities` — update source priorities and persist.
pub async fn put_source_priorities(
    State(state): State<Arc<ServerState>>,
    Json(priorities): Json<HashMap<String, u16>>,
) -> impl IntoResponse {
    state.config_store.set_source_priorities(priorities.clone());

    // Apply to the live store immediately
    state
        .store
        .write()
        .await
        .set_source_priorities(priorities);

    StatusCode::NO_CONTENT.into_response()
}

/// Stub handler returning `[]` for unimplemented list endpoints.
pub async fn empty_array() -> impl IntoResponse {
    Json(serde_json::json!([]))
}

/// Stub handler returning `{}` for unimplemented object endpoints.
pub async fn empty_object() -> impl IntoResponse {
    Json(serde_json::json!({}))
}

/// `GET /skServer/logfiles/` — log file listing (stub).
///
/// The admin UI fetches this on mount and calls `.json()` on the response.
/// A 404 causes `t.json is not a function`. Return empty array.
pub async fn list_logfiles() -> impl IntoResponse {
    Json(serde_json::json!([]))
}

/// `PUT /skServer/runDiscovery` — trigger webapp/plugin rediscovery (no-op stub).
pub async fn run_discovery() -> impl IntoResponse {
    Json(serde_json::json!({"message": "ok"}))
}

/// `GET /skServer/appstore/available` — app store listing (stub).
///
/// The admin UI Redux reducer directly accesses `.installing`, `.available`,
/// `.installed`, and `.updates` without null-checks, so this must return
/// an object with all four arrays present.
pub async fn appstore_available() -> impl IntoResponse {
    Json(serde_json::json!({
        "available": [],
        "installed": [],
        "installing": [],
        "updates": [],
    }))
}
