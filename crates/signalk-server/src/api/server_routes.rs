/// `/skServer/*` compatibility routes for the SignalK admin UI.
///
/// The admin UI sets `window.serverRoutesPrefix = '/skServer'` and calls all
/// API endpoints under this prefix. These handlers bridge to our existing
/// admin API or return static/stub responses.
use axum::{Json, extract::State, response::IntoResponse};
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

/// `GET /skServer/vessel` — vessel configuration.
pub async fn get_vessel(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let vessel = &state.config.vessel;
    Json(serde_json::json!({
        "name": vessel.name,
        "uuid": vessel.uuid,
        "mmsi": vessel.mmsi,
    }))
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
