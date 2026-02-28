/// Spec-compliant track API routes.
///
/// These handlers serve vessel track data at the paths that chart applications
/// (Freeboard, InstrumentPanel, etc.) expect:
///
/// - `GET    /signalk/v1/api/tracks`                   — all vessel tracks
/// - `GET    /signalk/v1/api/vessels/{vessel_id}/track` — specific vessel track
/// - `DELETE /signalk/v1/api/tracks`                   — clear all tracks
/// - `DELETE /signalk/v1/api/vessels/{vessel_id}/track` — clear specific vessel track
///
/// All handlers delegate to the tracks plugin's handlers in the
/// `PluginRouteTable`, transforming the URL path parameter into a
/// `?context=` query parameter that the plugin already supports.
use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use super::to_axum_response;
use crate::ServerState;

/// `GET /signalk/v1/api/tracks` — all vessel tracks (GeoJSON or GPX).
pub async fn get_all_tracks(
    State(state): State<Arc<ServerState>>,
    raw_query: RawQuery,
    headers: HeaderMap,
) -> Response {
    dispatch_to_tracks(&state, raw_query.0.as_deref(), &headers).await
}

/// `GET /signalk/v1/api/vessels/{vessel_id}/track` — specific vessel track.
///
/// `vessel_id` can be `self` (own vessel) or any vessel identifier (UUID, MMSI).
pub async fn get_vessel_track(
    State(state): State<Arc<ServerState>>,
    Path(vessel_id): Path<String>,
    raw_query: RawQuery,
    headers: HeaderMap,
) -> Response {
    let context = format!("vessels.{vessel_id}");
    let query = prepend_context(raw_query.0.as_deref(), &context);
    dispatch_to_tracks(&state, Some(&query), &headers).await
}

/// `DELETE /signalk/v1/api/tracks` — clear all tracks.
pub async fn delete_all_tracks(
    State(state): State<Arc<ServerState>>,
    raw_query: RawQuery,
    headers: HeaderMap,
) -> Response {
    dispatch_delete_tracks(&state, raw_query.0.as_deref(), &headers).await
}

/// `DELETE /signalk/v1/api/vessels/{vessel_id}/track` — clear specific vessel track.
pub async fn delete_vessel_track(
    State(state): State<Arc<ServerState>>,
    Path(vessel_id): Path<String>,
    raw_query: RawQuery,
    headers: HeaderMap,
) -> Response {
    let context = format!("vessels.{vessel_id}");
    let query = prepend_context(raw_query.0.as_deref(), &context);
    dispatch_delete_tracks(&state, Some(&query), &headers).await
}

/// Forward a DELETE request to the tracks plugin in the `PluginRouteTable`.
async fn dispatch_delete_tracks(
    state: &ServerState,
    query: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    let req_headers: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
        .collect();

    let req = signalk_plugin_api::PluginRequest {
        method: "DELETE".into(),
        path: "/".into(),
        query: query.map(String::from),
        headers: req_headers,
        body: vec![],
    };

    match state.route_table.handle("tracks", "DELETE", "/", req).await {
        Some(resp) => to_axum_response(resp),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"message": "Tracks plugin not running"})),
        )
            .into_response(),
    }
}

/// Forward a GET request to the tracks plugin in the `PluginRouteTable`.
async fn dispatch_to_tracks(
    state: &ServerState,
    query: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    let req_headers: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
        .collect();

    let req = signalk_plugin_api::PluginRequest {
        method: "GET".into(),
        path: "/".into(),
        query: query.map(String::from),
        headers: req_headers,
        body: vec![],
    };

    match state.route_table.handle("tracks", "GET", "/", req).await {
        Some(resp) => to_axum_response(resp),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"message": "Tracks plugin not running"})),
        )
            .into_response(),
    }
}

/// Prepend `context=...` to an existing query string.
fn prepend_context(existing: Option<&str>, context: &str) -> String {
    match existing {
        Some(q) if !q.is_empty() => format!("context={context}&{q}"),
        _ => format!("context={context}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepend_context_no_existing() {
        assert_eq!(
            prepend_context(None, "vessels.self"),
            "context=vessels.self"
        );
    }

    #[test]
    fn prepend_context_with_existing() {
        assert_eq!(
            prepend_context(Some("format=gpx&limit=100"), "vessels.self"),
            "context=vessels.self&format=gpx&limit=100"
        );
    }

    #[test]
    fn prepend_context_empty_string() {
        assert_eq!(
            prepend_context(Some(""), "vessels.self"),
            "context=vessels.self"
        );
    }
}
