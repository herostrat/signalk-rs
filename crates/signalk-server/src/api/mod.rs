/// REST API handlers for the SignalK HTTP interface.
///
/// Routes:
/// - GET /signalk                      → discovery
/// - GET /signalk/v1/api/*             → data model traversal
/// - PUT /signalk/v1/api/*             → PUT command (delegated to handlers)
/// - GET /signalk/v1/snapshot          → historical (501)
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use signalk_types::{DiscoveryResponse, EndpointInfo, ServerInfo};
use std::collections::HashMap;
use std::sync::Arc;

use crate::ServerState;

/// GET /signalk — server discovery endpoint.
///
/// Returns available API versions and their endpoints.
/// This is the entry point for all SignalK clients.
pub async fn discovery(State(state): State<Arc<ServerState>>) -> Response {
    let base = format!(
        "http://{}:{}/signalk/v1",
        state.config.server.host, state.config.server.port
    );
    let ws_base = format!(
        "ws://{}:{}/signalk/v1",
        state.config.server.host, state.config.server.port
    );

    let mut endpoints = HashMap::new();
    endpoints.insert(
        "v1".to_string(),
        EndpointInfo {
            version: signalk_types::SIGNALK_VERSION.to_string(),
            signalk_http: base.clone(),
            signalk_ws: format!("{}/stream", ws_base),
            signalk_tcp: None,
        },
    );

    let resp = DiscoveryResponse {
        endpoints,
        server: ServerInfo {
            id: "signalk-rs".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    };

    Json(resp).into_response()
}

/// GET /signalk/v1/api/ — full data model snapshot.
pub async fn full_model(State(state): State<Arc<ServerState>>) -> Response {
    let store = state.store.read().await;
    let model = store.full_model();
    Json(model).into_response()
}

/// GET /signalk/v1/api/{*path} — hierarchical path traversal.
///
/// The path is dot-separated in the data model but slash-separated in the URL.
/// e.g. GET /signalk/v1/api/vessels/self/navigation/speedOverGround
///      → path: "navigation.speedOverGround" in self vessel context
pub async fn get_path(
    State(state): State<Arc<ServerState>>,
    Path(url_path): Path<String>,
) -> Response {
    let store = state.store.read().await;

    let raw_parts: Vec<&str> = url_path.split('/').filter(|s| !s.is_empty()).collect();

    if raw_parts.is_empty() {
        return Json(store.full_model()).into_response();
    }

    // Resolve "vessels/self/..." → "vessels/{self_uri}/..."
    // "self" is a spec-defined alias for the local vessel.
    let self_uri = store.self_uri.clone();
    let parts: Vec<String> = raw_parts
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            if p == "self" && i == 1 && raw_parts.first().copied() == Some("vessels") {
                self_uri.clone()
            } else {
                p.to_string()
            }
        })
        .collect();

    // Build JSON response by traversing the full model
    let model_value = match serde_json::to_value(store.full_model()) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": e.to_string()})),
            )
                .into_response();
        }
    };

    let parts_ref: Vec<&str> = parts.iter().map(String::as_str).collect();
    match traverse_json(&model_value, &parts_ref) {
        Some(value) => Json(value.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"message": format!("Path not found: {}", url_path)})),
        )
            .into_response(),
    }
}

/// GET /signalk/v1/snapshot — historical data (not implemented).
pub async fn snapshot() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({"message": "Historical data not supported"})),
    )
        .into_response()
}

/// PUT /signalk/v1/api/{*path} — send a command/write request.
///
/// Delegates to registered PUT handlers. Forwards to the bridge via UDS.
/// Returns 404 if no handler is registered, 503 if the bridge is unreachable.
pub async fn put_path(
    State(state): State<Arc<ServerState>>,
    Path(url_path): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let parts: Vec<&str> = url_path.split('/').filter(|s| !s.is_empty()).collect();

    // Convert URL path segments to dot-path
    // e.g. vessels/self/steering/autopilot/target/headingTrue
    //   → steering.autopilot.target.headingTrue (after vessels/self prefix)
    let sk_path = if parts.len() >= 2 && parts[0] == "vessels" {
        // Skip "vessels" and vessel-id segments
        parts[2..].join(".")
    } else {
        parts.join(".")
    };

    let handlers = state.put_handlers.read().await;

    // Find a registered handler for this path (map: pattern → plugin_id)
    let plugin_id = handlers.iter().find_map(|(pattern, id)| {
        if signalk_types::matches_pattern(pattern, &sk_path) {
            Some(id.clone())
        } else {
            None
        }
    });
    drop(handlers);

    match plugin_id {
        Some(plugin_id) => {
            let request_id = uuid::Uuid::new_v4().to_string();
            // SignalK PUT body is { "value": X }; extract the inner value to forward.
            let sk_value = body.get("value").cloned().unwrap_or(body);
            let bridge_path = format!("/put/{}/{}", plugin_id, sk_path.replace('.', "/"));
            let bridge_socket = std::path::Path::new(&state.config.internal.uds_bridge_socket);
            let payload = serde_json::json!({
                "requestId": request_id,
                "value": sk_value,
            });

            match signalk_internal::uds::uds_post(bridge_socket, &bridge_path, &payload).await {
                Ok(resp) => {
                    let put_state = resp["state"].as_str().unwrap_or("FAILED");
                    let put_code = resp["statusCode"].as_u64().unwrap_or(500) as u16;
                    let http_status = if put_state == "COMPLETED" {
                        StatusCode::OK
                    } else {
                        StatusCode::INTERNAL_SERVER_ERROR
                    };
                    (
                        http_status,
                        Json(serde_json::json!({
                            "state": put_state,
                            "statusCode": put_code,
                        })),
                    )
                        .into_response()
                }
                Err(e) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"message": format!("Bridge unreachable: {e}")})),
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "message": format!("No handler registered for path: {}", sk_path)
            })),
        )
            .into_response(),
    }
}

/// ANY /plugins/{plugin_id}[/{*rest}] — reverse-proxy to the bridge plugin router.
///
/// Plugins register their REST routes via `registerWithRouter()`. signalk-rs proxies
/// all requests under `/plugins/{plugin_id}/` to the bridge, which dispatches to the
/// plugin's Express router.
///
/// Returns 404 if the plugin has no registered route prefix, 503 if the bridge is
/// unreachable.
pub async fn proxy_plugin_route(
    State(state): State<Arc<ServerState>>,
    request: axum::extract::Request,
) -> Response {
    // Extract plugin_id from URI: /plugins/{plugin_id}[/{rest}]
    let uri = request.uri().clone();
    let path = uri.path();
    let plugin_id = path
        .strip_prefix("/plugins/")
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();

    if plugin_id.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"message": "Missing plugin id in path"})),
        )
            .into_response();
    }

    {
        let routes = state.plugin_routes.read().await;
        if !routes.contains_key(&plugin_id) {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "message": format!("No route registered for plugin: {}", plugin_id)
                })),
            )
                .into_response();
        }
    }

    let method = request.method().as_str().to_string();
    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| path.to_string());

    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let body_bytes = match axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"message": format!("Failed to read request body: {e}")})),
            )
                .into_response();
        }
    };

    let bridge_socket = std::path::Path::new(&state.config.internal.uds_bridge_socket);

    match signalk_internal::uds::uds_proxy(
        bridge_socket,
        &method,
        &path_and_query,
        content_type.as_deref(),
        &body_bytes,
    )
    .await
    {
        Ok((status, body, resp_ct)) => {
            let mut builder =
                axum::response::Response::builder().status(status);
            if let Some(ct) = resp_ct {
                builder = builder.header(header::CONTENT_TYPE, ct);
            }
            builder
                .body(Body::from(body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"message": format!("Bridge unreachable: {e}")})),
        )
            .into_response(),
    }
}

/// Recursively traverse a JSON value using URL path segments.
fn traverse_json<'a>(value: &'a Value, parts: &[&str]) -> Option<&'a Value> {
    if parts.is_empty() {
        return Some(value);
    }

    match value {
        Value::Object(map) => {
            // Try exact key match first
            if let Some(child) = map.get(parts[0]) {
                return traverse_json(child, &parts[1..]);
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn traverse_json_exact() {
        let v = json!({"vessels": {"self": {"navigation": {"speedOverGround": {"value": 3.5}}}}});
        let result = traverse_json(&v, &["vessels", "self", "navigation", "speedOverGround"]);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["value"], 3.5);
    }

    #[test]
    fn traverse_json_missing() {
        let v = json!({"vessels": {}});
        let result = traverse_json(&v, &["vessels", "self"]);
        assert!(result.is_none());
    }

    #[test]
    fn traverse_json_root() {
        let v = json!({"version": "1.7.0"});
        let result = traverse_json(&v, &[]);
        assert_eq!(result.unwrap(), &v);
    }
}
