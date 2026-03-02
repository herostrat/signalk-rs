/// SignalK V2 Autopilot API handlers.
///
/// Spec: https://demo.signalk.org/documentation/develop/rest-api/autopilot_api.html
///
/// All endpoints delegate to the `AutopilotManager`, which forwards commands
/// to registered `AutopilotProvider` plugins.
///
/// Device ID `_default` resolves to whichever provider is currently default.
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use signalk_plugin_api::TackDirection;
use signalk_types::{Delta, PathValue, Source, Update};
use std::sync::Arc;

use crate::ServerState;

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Convert degrees to radians if `units == "deg"`, otherwise pass through.
fn to_radians(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("deg") => value.to_radians(),
        _ => value,
    }
}

/// Resolve device ID (may be `"_default"`) to an actual provider.
/// Returns `404` if not found.
macro_rules! resolve_provider {
    ($state:expr, $device_id:expr) => {{
        let id = $state.autopilot_manager.resolve_id($device_id).await;
        match id {
            Some(real_id) => match $state.autopilot_manager.get(&real_id).await {
                Some(provider) => provider,
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(json!({"message": format!("Autopilot not found: {}", $device_id)})),
                    )
                        .into_response()
                }
            },
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"message": format!("Autopilot not found: {}", $device_id)})),
                )
                    .into_response()
            }
        }
    }};
}

/// Convert a `PluginError` to an HTTP error response.
fn plugin_err(e: signalk_plugin_api::PluginError) -> Response {
    if e.is_not_found() {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"message": e.to_string()})),
        )
            .into_response()
    } else if e.is_bad_request() {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": e.to_string()})),
        )
            .into_response()
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"message": e.to_string()})),
        )
            .into_response()
    }
}

/// Parse a tack/gybe direction from the URL path segment.
fn parse_direction(s: &str) -> Option<TackDirection> {
    match s.to_lowercase().as_str() {
        "port" => Some(TackDirection::Port),
        "starboard" => Some(TackDirection::Starboard),
        _ => None,
    }
}

/// 400 response for invalid tack/gybe direction.
fn bad_direction(s: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"message": format!("Invalid direction: '{s}'. Use 'port' or 'starboard'.")})),
    )
        .into_response()
}

// ─── Discovery ────────────────────────────────────────────────────────────────

/// GET /signalk/v2/api/vessels/self/autopilots
///
/// Spec: returns a flat object `{ "device-id": { "provider": "plugin-name", "isDefault": true } }`.
pub async fn list_autopilots(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let entries = state.autopilot_manager.list().await;

    let mut map = serde_json::Map::new();
    for (id, provider_name, is_default) in entries {
        map.insert(
            id,
            json!({
                "provider": provider_name,
                "isDefault": is_default,
            }),
        );
    }
    Json(Value::Object(map))
}

// ─── Default provider ─────────────────────────────────────────────────────────

/// GET /signalk/v2/api/vessels/self/autopilots/_providers/_default
pub async fn get_default_provider(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    match state.autopilot_manager.default_id().await {
        Some(id) => Json(json!({"id": id})).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"message": "No autopilot registered"})),
        )
            .into_response(),
    }
}

/// POST /signalk/v2/api/vessels/self/autopilots/_providers/_default/{id}
pub async fn set_default_provider(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.autopilot_manager.set_default(&id).await {
        Ok(()) => {
            // Emit defaultPilot delta so WS subscribers see the change
            let delta = Delta::self_vessel(vec![Update::new(
                Source::plugin("autopilot-api"),
                vec![PathValue::new("steering.autopilot.defaultPilot", json!(id))],
            )]);
            state.store.write().await.apply_delta(delta);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => plugin_err(e),
    }
}

// ─── Device state ─────────────────────────────────────────────────────────────

/// GET /signalk/v2/api/vessels/self/autopilots/{device_id}
pub async fn get_autopilot(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    let provider = resolve_provider!(state, &device_id);
    match provider.get_data().await {
        Ok(data) => Json(data).into_response(),
        Err(e) => plugin_err(e),
    }
}

/// GET /signalk/v2/api/vessels/self/autopilots/{device_id}/state
pub async fn get_state(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    let provider = resolve_provider!(state, &device_id);
    match provider.get_state().await {
        Ok(s) => Json(json!({"value": s})).into_response(),
        Err(e) => plugin_err(e),
    }
}

/// PUT /signalk/v2/api/vessels/self/autopilots/{device_id}/state
/// Body: `{"value": "enabled"}` or `{"value": "disabled"}`
pub async fn set_state(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let value = match body.get("value").and_then(|v| v.as_str()) {
        Some(v) => v.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"message": "Missing 'value' field"})),
            )
                .into_response();
        }
    };
    let provider = resolve_provider!(state, &device_id);
    match provider.set_state(&value).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

// ─── Mode ─────────────────────────────────────────────────────────────────────

/// GET /signalk/v2/api/vessels/self/autopilots/{device_id}/mode
pub async fn get_mode(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    let provider = resolve_provider!(state, &device_id);
    match provider.get_mode().await {
        Ok(m) => Json(json!({"value": m})).into_response(),
        Err(e) => plugin_err(e),
    }
}

/// PUT /signalk/v2/api/vessels/self/autopilots/{device_id}/mode
/// Body: `{"value": "compass"}`
pub async fn set_mode(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mode = match body.get("value").and_then(|v| v.as_str()) {
        Some(v) => v.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"message": "Missing 'value' field"})),
            )
                .into_response();
        }
    };
    let provider = resolve_provider!(state, &device_id);
    match provider.set_mode(&mode).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

// ─── Target ───────────────────────────────────────────────────────────────────

/// GET /signalk/v2/api/vessels/self/autopilots/{device_id}/target
pub async fn get_target(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    let provider = resolve_provider!(state, &device_id);
    match provider.get_target().await {
        Ok(t) => Json(json!({"value": t})).into_response(),
        Err(e) => plugin_err(e),
    }
}

/// PUT /signalk/v2/api/vessels/self/autopilots/{device_id}/target
/// Body: `{"value": 1.52}` (radians) or `{"value": 90, "units": "deg"}`
pub async fn set_target(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let raw = match body.get("value").and_then(|v| v.as_f64()) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"message": "Missing or non-numeric 'value' field"})),
            )
                .into_response();
        }
    };
    let units = body.get("units").and_then(|u| u.as_str());
    let value_rad = to_radians(raw, units);
    let provider = resolve_provider!(state, &device_id);
    match provider.set_target(value_rad).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

/// PUT /signalk/v2/api/vessels/self/autopilots/{device_id}/target/adjust
/// Body: `{"value": -5, "units": "deg"}`
pub async fn adjust_target(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let raw = match body.get("value").and_then(|v| v.as_f64()) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"message": "Missing or non-numeric 'value' field"})),
            )
                .into_response();
        }
    };
    let units = body.get("units").and_then(|u| u.as_str());
    let delta_rad = to_radians(raw, units);
    let provider = resolve_provider!(state, &device_id);
    match provider.adjust_target(delta_rad).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

// ─── Engagement ───────────────────────────────────────────────────────────────

/// POST /signalk/v2/api/vessels/self/autopilots/{device_id}/engage
pub async fn engage(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    let provider = resolve_provider!(state, &device_id);
    match provider.engage().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

/// POST /signalk/v2/api/vessels/self/autopilots/{device_id}/disengage
pub async fn disengage(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    let provider = resolve_provider!(state, &device_id);
    match provider.disengage().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

// ─── Maneuvers ────────────────────────────────────────────────────────────────

/// POST /signalk/v2/api/vessels/self/autopilots/{device_id}/tack/{direction}
pub async fn tack(
    State(state): State<Arc<ServerState>>,
    Path((device_id, direction)): Path<(String, String)>,
) -> Response {
    let dir = match parse_direction(&direction) {
        Some(d) => d,
        None => return bad_direction(&direction),
    };
    let provider = resolve_provider!(state, &device_id);
    match provider.tack(dir).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

/// POST /signalk/v2/api/vessels/self/autopilots/{device_id}/gybe/{direction}
pub async fn gybe(
    State(state): State<Arc<ServerState>>,
    Path((device_id, direction)): Path<(String, String)>,
) -> Response {
    let dir = match parse_direction(&direction) {
        Some(d) => d,
        None => return bad_direction(&direction),
    };
    let provider = resolve_provider!(state, &device_id);
    match provider.gybe(dir).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

// ─── Dodge mode ───────────────────────────────────────────────────────────────

/// POST /signalk/v2/api/vessels/self/autopilots/{device_id}/dodge
/// Enters dodge mode without an initial offset (offset = 0.0).
pub async fn dodge_enter(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    let provider = resolve_provider!(state, &device_id);
    match provider.dodge(Some(0.0)).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

/// PUT /signalk/v2/api/vessels/self/autopilots/{device_id}/dodge
/// Body: `{"value": 5, "units": "deg"}` — adjust dodge offset.
pub async fn dodge_adjust(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let raw = match body.get("value").and_then(|v| v.as_f64()) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"message": "Missing or non-numeric 'value' field"})),
            )
                .into_response();
        }
    };
    let units = body.get("units").and_then(|u| u.as_str());
    let offset_rad = to_radians(raw, units);
    let provider = resolve_provider!(state, &device_id);
    match provider.dodge(Some(offset_rad)).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

/// DELETE /signalk/v2/api/vessels/self/autopilots/{device_id}/dodge
/// Exits dodge mode, returning to original target.
pub async fn dodge_exit(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    let provider = resolve_provider!(state, &device_id);
    match provider.dodge(None).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

// ─── Course operations ────────────────────────────────────────────────────────

/// POST /signalk/v2/api/vessels/self/autopilots/{device_id}/courseCurrentPoint
///
/// Activate course-following mode: set Route mode + engage towards the current
/// course destination. Requires an active course (nextPoint) in nav state.
pub async fn course_current_point(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    let provider = resolve_provider!(state, &device_id);
    match provider.course_current_point().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}

/// POST /signalk/v2/api/vessels/self/autopilots/{device_id}/courseNextPoint
///
/// Advance to the next waypoint on the active route, then ensure the autopilot
/// remains engaged in route mode.
///
/// Coordination: the handler advances the course via CourseManager, then the
/// provider ensures route mode stays active.
pub async fn course_next_point(
    State(state): State<Arc<ServerState>>,
    Path(device_id): Path<String>,
) -> Response {
    // Advance the course waypoint first (server-level coordination)
    if let Err(e) = state.course_manager.advance_next_point(1).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": format!("Failed to advance waypoint: {e}")})),
        )
            .into_response();
    }

    // Then tell the provider to ensure route mode is active
    let provider = resolve_provider!(state, &device_id);
    match provider.course_next_point().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => plugin_err(e),
    }
}
