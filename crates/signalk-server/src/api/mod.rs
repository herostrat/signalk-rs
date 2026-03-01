pub mod admin;
pub mod server_routes;
pub mod tracks;
pub mod v2;
pub mod webapps;

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

/// Convert a framework-agnostic `PluginResponse` into an axum `Response`.
///
/// Used by plugin route dispatch and spec-level routes that delegate to plugins.
pub fn to_axum_response(resp: signalk_plugin_api::PluginResponse) -> Response {
    let mut builder = axum::response::Response::builder().status(resp.status);
    for (k, v) in &resp.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    builder
        .body(Body::from(resp.body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// GET /signalk — server discovery endpoint.
///
/// Returns available API versions and their endpoints.
/// This is the entry point for all SignalK clients.
pub async fn discovery(
    State(_state): State<Arc<ServerState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    // Use the request Host header so webapps can reach us from the browser.
    // Falls back to config host:port if no Host header is present.
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:3000");
    let base = format!("http://{host}/signalk/v1");
    let ws_base = format!("ws://{host}/signalk/v1");

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

/// Query parameters for GET /signalk/v1/api/{*path}.
#[derive(Debug, serde::Deserialize)]
pub struct PathQueryParams {
    /// Select a specific source instead of the highest-priority one.
    #[serde(default)]
    source: Option<String>,
}

/// GET /signalk/v1/api/{*path} — hierarchical path traversal.
///
/// The path is dot-separated in the data model but slash-separated in the URL.
/// e.g. GET /signalk/v1/api/vessels/self/navigation/speedOverGround
///      → path: "navigation.speedOverGround" in self vessel context
///
/// Optional query parameter `?source=gps.GP` to select a specific data source.
pub async fn get_path(
    State(state): State<Arc<ServerState>>,
    Path(url_path): Path<String>,
    axum::extract::Query(query): axum::extract::Query<PathQueryParams>,
) -> Response {
    let store = state.store.read().await;

    let raw_parts: Vec<&str> = url_path.split('/').filter(|s| !s.is_empty()).collect();

    if raw_parts.is_empty() {
        return Json(store.full_model()).into_response();
    }

    // Source-specific query: return the value from a named source
    if let Some(ref source_ref) = query.source {
        // Convert URL path to dot-path for store lookup
        let sk_path = if raw_parts.len() >= 2 && raw_parts[0] == "vessels" {
            raw_parts[2..].join(".")
        } else {
            raw_parts.join(".")
        };

        return match store.get_self_path_by_source(&sk_path, source_ref) {
            Some(val) => Json(val).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "message": format!("No value for {sk_path} from source {source_ref}")
                })),
            )
                .into_response(),
        };
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

/// Generic 501 Not Implemented handler for spec-defined routes not yet supported.
pub async fn not_implemented() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({"message": "Not implemented"})),
    )
        .into_response()
}

/// PUT /signalk/v1/api/{*path} — send a command/write request.
///
/// Checks Tier 1 (local Rust) handlers first, then falls back to Tier 2 (bridge).
/// Returns 404 if no handler is registered, 503 if the bridge is unreachable.
///
/// Special case: if the path ends with `/meta`, writes metadata for the parent path.
pub async fn put_path(
    State(state): State<Arc<ServerState>>,
    Path(url_path): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let parts: Vec<&str> = url_path.split('/').filter(|s| !s.is_empty()).collect();

    let sk_path = if parts.len() >= 2 && parts[0] == "vessels" {
        parts[2..].join(".")
    } else {
        parts.join(".")
    };

    // ── Metadata PUT: path ends with /meta ──────────────────────────────
    if parts.last() == Some(&"meta") {
        let data_path = if let Some(stripped) = sk_path.strip_suffix(".meta") {
            stripped
        } else {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"message": "Invalid meta path"})),
            )
                .into_response();
        };

        let meta: signalk_types::Metadata = match serde_json::from_value(body) {
            Ok(m) => m,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"message": format!("Invalid metadata: {e}")})),
                )
                    .into_response();
            }
        };

        let mut store = state.store.write().await;
        store.set_metadata(data_path, meta);
        return Json(serde_json::json!({"state": "COMPLETED", "statusCode": 200})).into_response();
    }

    let request_id = uuid::Uuid::new_v4().to_string();
    let sk_value = body.get("value").cloned().unwrap_or(body);

    // ── Tier 1: check local Rust PUT handlers first ─────────────────────
    if let Some((_plugin_id, handler)) = state.put_handler_registry.find(&sk_path).await {
        let cmd = signalk_plugin_api::PutCommand {
            path: sk_path.clone(),
            value: sk_value.clone(),
            source: None,
            request_id: request_id.clone(),
        };
        match handler(cmd).await {
            Ok(signalk_plugin_api::PutHandlerResult::Completed) => {
                return Json(serde_json::json!({"state": "COMPLETED", "statusCode": 200}))
                    .into_response();
            }
            Ok(signalk_plugin_api::PutHandlerResult::Pending) => {
                return (
                    StatusCode::ACCEPTED,
                    Json(serde_json::json!({"state": "PENDING", "statusCode": 202})),
                )
                    .into_response();
            }
            Ok(signalk_plugin_api::PutHandlerResult::Failed(msg)) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"state": "FAILED", "statusCode": 500, "message": msg})),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"state": "FAILED", "statusCode": 500, "message": e.to_string()})),
                )
                    .into_response();
            }
        }
    }

    // ── Tier 2: fall back to bridge handlers ────────────────────────────
    let handlers = state.put_handlers.read().await;
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

/// ANY /plugins/{plugin_id}[/{*rest}] — plugin route dispatch.
///
/// Checks Tier 1 (local Rust) routes first, then falls back to Tier 2 (bridge proxy).
/// Returns 404 if the plugin has no registered routes, 503 if the bridge is unreachable.
pub async fn proxy_plugin_route(
    State(state): State<Arc<ServerState>>,
    request: axum::extract::Request,
) -> Response {
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

    let method = request.method().as_str().to_string();
    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| path.to_string());
    let query = uri.query().map(str::to_string);

    // Compute the relative path (after /plugins/{plugin_id})
    let prefix = format!("/plugins/{}", plugin_id);
    let relative_path = path.strip_prefix(&prefix).unwrap_or("/").to_string();
    let relative_path = if relative_path.is_empty() {
        "/".to_string()
    } else {
        relative_path
    };

    // Extract all request info before consuming the body
    let req_headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
        .collect();
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
                Json(serde_json::json!({"message": format!("Failed to read body: {e}")})),
            )
                .into_response();
        }
    };

    // ── Tier 1: check local Rust plugin routes first ────────────────────
    if state.route_table.has_routes(&plugin_id).await {
        let plugin_req = signalk_plugin_api::PluginRequest {
            method: method.clone(),
            path: relative_path.clone(),
            query: query.clone(),
            headers: req_headers.clone(),
            body: body_bytes.to_vec(),
        };

        if let Some(plugin_resp) = state
            .route_table
            .handle(&plugin_id, &method, &relative_path, plugin_req)
            .await
        {
            return to_axum_response(plugin_resp);
        }
    }

    // ── Tier 2: fall back to bridge proxy ───────────────────────────────
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
            let mut builder = axum::response::Response::builder().status(status);
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

// ── applicationData ─────────────────────────────────────────────────────────

/// Resolve the file path for applicationData.
/// Scope "user" falls back to "global" (no auth system yet).
fn app_data_dir(data_dir: &std::path::Path, scope: &str, app_id: &str) -> std::path::PathBuf {
    let effective_scope = if scope == "user" { "global" } else { scope };
    data_dir
        .join("applicationData")
        .join(effective_scope)
        .join(app_id)
}

/// Validate scope parameter — only "global" and "user" are allowed.
/// Returns `Some(error_response)` if invalid, `None` if valid.
fn invalid_scope(scope: &str) -> Option<Response> {
    if scope != "global" && scope != "user" {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": format!("Invalid scope: {scope}. Must be 'global' or 'user'")})),
        )
            .into_response());
    }
    None
}

/// GET /signalk/v1/applicationData/{scope}/{appId}/{version} — read application data.
pub async fn get_app_data(
    State(state): State<Arc<ServerState>>,
    Path((scope, app_id, version)): Path<(String, String, String)>,
) -> Response {
    if let Some(r) = invalid_scope(&scope) {
        return r;
    }
    let file_path = app_data_dir(&state.data_dir, &scope, &app_id).join(format!("{version}.json"));

    match tokio::fs::read_to_string(&file_path).await {
        Ok(contents) => match serde_json::from_str::<Value>(&contents) {
            Ok(v) => Json(v).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": format!("Corrupt data file: {e}")})),
            )
                .into_response(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"message": format!("No data for {app_id}/{version}")})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": format!("Read error: {e}")})),
        )
            .into_response(),
    }
}

/// POST /signalk/v1/applicationData/{scope}/{appId}/{version} — store application data.
pub async fn set_app_data(
    State(state): State<Arc<ServerState>>,
    Path((scope, app_id, version)): Path<(String, String, String)>,
    Json(body): Json<Value>,
) -> Response {
    if let Some(r) = invalid_scope(&scope) {
        return r;
    }
    // Validate: appId and version must be simple names (no path traversal)
    if app_id.contains('/')
        || app_id.contains("..")
        || version.contains('/')
        || version.contains("..")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": "Invalid appId or version"})),
        )
            .into_response();
    }

    let dir = app_data_dir(&state.data_dir, &scope, &app_id);
    let file_path = dir.join(format!("{version}.json"));

    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": format!("Cannot create directory: {e}")})),
        )
            .into_response();
    }

    let contents = match serde_json::to_string_pretty(&body) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": format!("Serialization error: {e}")})),
            )
                .into_response();
        }
    };

    match tokio::fs::write(&file_path, contents).await {
        Ok(()) => {
            Json(serde_json::json!({"state": "COMPLETED", "statusCode": 200})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": format!("Write error: {e}")})),
        )
            .into_response(),
    }
}

/// GET /signalk/v1/applicationData/{scope}/{appId}/{version}/{*key} — read sub-key.
pub async fn get_app_data_key(
    State(state): State<Arc<ServerState>>,
    Path((scope, app_id, version, key)): Path<(String, String, String, String)>,
) -> Response {
    if let Some(r) = invalid_scope(&scope) {
        return r;
    }
    let file_path = app_data_dir(&state.data_dir, &scope, &app_id).join(format!("{version}.json"));

    let contents = match tokio::fs::read_to_string(&file_path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"message": format!("No data for {app_id}/{version}")})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": format!("Read error: {e}")})),
            )
                .into_response();
        }
    };

    let root: Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": format!("Corrupt data file: {e}")})),
            )
                .into_response();
        }
    };

    let parts: Vec<&str> = key.split('/').filter(|s| !s.is_empty()).collect();
    match traverse_json(&root, &parts) {
        Some(value) => Json(value.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"message": format!("Key not found: {key}")})),
        )
            .into_response(),
    }
}

/// POST /signalk/v1/applicationData/{scope}/{appId}/{version}/{*key} — write sub-key.
pub async fn set_app_data_key(
    State(state): State<Arc<ServerState>>,
    Path((scope, app_id, version, key)): Path<(String, String, String, String)>,
    Json(body): Json<Value>,
) -> Response {
    if let Some(r) = invalid_scope(&scope) {
        return r;
    }
    if app_id.contains('/')
        || app_id.contains("..")
        || version.contains('/')
        || version.contains("..")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": "Invalid appId or version"})),
        )
            .into_response();
    }

    let dir = app_data_dir(&state.data_dir, &scope, &app_id);
    let file_path = dir.join(format!("{version}.json"));

    // Read existing data (or start with empty object)
    let mut root: Value = match tokio::fs::read_to_string(&file_path).await {
        Ok(c) => serde_json::from_str(&c).unwrap_or(Value::Object(Default::default())),
        Err(_) => Value::Object(Default::default()),
    };

    // Set the nested key
    let parts: Vec<&str> = key.split('/').filter(|s| !s.is_empty()).collect();
    set_nested(&mut root, &parts, body);

    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": format!("Cannot create directory: {e}")})),
        )
            .into_response();
    }

    let contents = serde_json::to_string_pretty(&root).unwrap_or_default();
    match tokio::fs::write(&file_path, contents).await {
        Ok(()) => {
            Json(serde_json::json!({"state": "COMPLETED", "statusCode": 200})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": format!("Write error: {e}")})),
        )
            .into_response(),
    }
}

/// Set a value at a nested path within a JSON value.
fn set_nested(root: &mut Value, parts: &[&str], value: Value) {
    if parts.is_empty() {
        *root = value;
        return;
    }
    let Some((head, tail)) = parts.split_first() else {
        return;
    };
    if !root.is_object() {
        *root = Value::Object(Default::default());
    }
    let obj = root.as_object_mut().unwrap();
    if tail.is_empty() {
        obj.insert((*head).to_string(), value);
    } else {
        let child = obj
            .entry((*head).to_string())
            .or_insert_with(|| Value::Object(Default::default()));
        set_nested(child, tail, value);
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

/// POST /test/inject — accept a raw delta and apply it to the store.
///
/// Only available when the `simulator` feature is enabled (development/testing builds).
/// Used by the conformance test runner to inject identical data into both servers.
#[cfg(feature = "simulator")]
pub async fn test_inject_delta(
    State(state): State<Arc<ServerState>>,
    Json(delta): Json<signalk_types::Delta>,
) -> Response {
    let mut store = state.store.write().await;
    store.apply_delta(delta);
    Json(serde_json::json!({"success": true})).into_response()
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
