//! Test helpers: build a ready-to-use app and make requests against it.
//!
//! Each test file includes this module and uses only the subset it needs.
//! `dead_code` is suppressed because the full helper surface is intentionally
//! shared across multiple test compilation units.
#![allow(dead_code)]

use axum::{Router, body::Body, http::Request, response::Response};
use signalk_server::{ServerState, build_router, config::ServerConfig};
use signalk_store::store::SignalKStore;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

/// Build a test app with default config and empty store.
pub fn test_app() -> Router {
    let config = ServerConfig::default();
    let (store, _rx) = SignalKStore::new(config.vessel.uuid.clone());
    let state = ServerState::new(config, store);
    build_router(state)
}

/// Build a test app AND return the `Arc<RwLock<SignalKStore>>` for data injection.
///
/// Tests inject deltas with `store.write().await.apply_delta(...)` before making
/// HTTP requests, verifying the full store → REST pipeline.
#[allow(dead_code)]
pub fn test_app_with_store() -> (Router, Arc<RwLock<SignalKStore>>) {
    let config = ServerConfig::default();
    let (store, _rx) = SignalKStore::new(config.vessel.uuid.clone());
    let state = ServerState::new(config, store.clone());
    (build_router(state), store)
}

/// Build a test app AND return the self vessel URI for assertions.
#[allow(dead_code)]
pub fn test_app_with_uri() -> (Router, String) {
    let config = ServerConfig::default();
    let self_uri = config.vessel.uuid.clone();
    let (store, _rx) = SignalKStore::new(&self_uri);
    let state = ServerState::new(config, store);
    (build_router(state), self_uri)
}

/// Make a GET request and return the parsed JSON body + status code.
pub async fn get(app: Router, uri: &str) -> (u16, serde_json::Value) {
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    response_json(response).await
}

/// Make a POST request with JSON body.
pub async fn post_json(
    app: Router,
    uri: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let response = app
        .oneshot(
            Request::post(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    response_json(response).await
}

/// Make a PUT request with JSON body.
#[allow(dead_code)]
pub async fn put_json(
    app: Router,
    uri: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let response = app
        .oneshot(
            Request::put(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    response_json(response).await
}

/// Build a test app with a pre-registered PUT handler pointing to a given plugin.
///
/// `handler_path` is a dot-notation path pattern, e.g. `"steering.autopilot.target.*"`.
/// `plugin_id` is the plugin identifier, e.g. `"test-plugin"`.
/// `bridge_socket` is the UDS socket path the server will try to forward to.
#[allow(dead_code)]
pub fn test_app_with_handler(handler_path: &str, plugin_id: &str, bridge_socket: &str) -> Router {
    use signalk_server::config::InternalSettings;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let config = ServerConfig {
        internal: InternalSettings {
            transport: "uds".to_string(),
            uds_rs_socket: "/tmp/test-rs.sock".to_string(),
            uds_bridge_socket: bridge_socket.to_string(),
            http_rs_port: 3001,
            http_bridge_port: 3002,
            bridge_token: String::new(),
        },
        ..ServerConfig::default()
    };
    let (store, _rx) = SignalKStore::new(config.vessel.uuid.clone());
    let put_handlers = Arc::new(RwLock::new(HashMap::from([(
        handler_path.to_string(),
        plugin_id.to_string(),
    )])));
    let plugin_routes: Arc<RwLock<HashMap<String, String>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let state = signalk_server::ServerState::new_shared(
        config,
        store,
        put_handlers,
        plugin_routes,
        Arc::new(signalk_server::plugins::host::PutHandlerRegistry::new()),
        Arc::new(signalk_server::plugins::routes::PluginRouteTable::new()),
    );
    build_router(state)
}

/// Make a GET request with a Bearer token.
#[allow(dead_code)]
pub async fn get_auth(app: Router, uri: &str, token: &str) -> (u16, serde_json::Value) {
    let response = app
        .oneshot(
            Request::get(uri)
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    response_json(response).await
}

async fn response_json(response: Response) -> (u16, serde_json::Value) {
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&bytes).to_string())
        })
    };
    (status, json)
}

/// Load a SignalK JSON schema from the embedded test schemas directory.
/// Returns `None` if the schema file is not found.
pub fn load_schema(name: &str) -> Option<serde_json::Value> {
    // Walk up from the crate dir to find the workspace tests/schemas/ directory
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let schema_path = manifest_dir
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .map(|root| {
            root.join("tests")
                .join("schemas")
                .join(format!("{}.json", name))
        })?;

    let content = std::fs::read_to_string(schema_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Assert that a JSON value conforms to a named SignalK schema.
/// Panics with a descriptive message if validation fails.
///
/// NOTE: Must be called from a non-async context (plain `#[test]`) or from a
/// thread spawned with `std::thread::spawn` to avoid tokio runtime conflicts.
pub fn assert_valid_schema(name: &str, value: &serde_json::Value) {
    let schema = match load_schema(name) {
        Some(s) => s,
        None => {
            eprintln!(
                "[schema] Warning: schema '{}' not found, skipping validation",
                name
            );
            return;
        }
    };

    // Pre-register all local schemas by their declared `id` URI so that
    // cross-references between SignalK schemas are resolved from disk instead
    // of triggering network requests (which panic inside a tokio executor).
    let mut opts = jsonschema::options();
    for &sname in &["delta", "signalk", "vessel", "definitions"] {
        let Some(s) = load_schema(sname) else {
            continue;
        };
        // Extract and own the URI string before `s` is moved into from_contents.
        let Some(uri) = s
            .get("id")
            .and_then(|v| v.as_str())
            .map(|id| id.trim_end_matches('#').to_string())
        else {
            continue;
        };
        let Ok(resource) = jsonschema::Resource::from_contents(s) else {
            continue;
        };
        opts = opts.with_resource(&uri, resource);
    }

    let compiled = match opts.build(&schema) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[schema] Warning: could not compile schema '{}': {} — skipping validation",
                name, e
            );
            return;
        }
    };

    let errors: Vec<_> = compiled.iter_errors(value).collect();
    if !errors.is_empty() {
        let messages: Vec<String> = errors
            .iter()
            .map(|e| format!("  - {} (path: {})", e, e.instance_path))
            .collect();
        panic!(
            "JSON Schema '{}' validation failed:\n{}\n\nValue:\n{}",
            name,
            messages.join("\n"),
            serde_json::to_string_pretty(value).unwrap()
        );
    }
}
