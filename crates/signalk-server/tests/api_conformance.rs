// Conformance tests for the SignalK REST API.
//
// These tests run in-process using axum's `oneshot` pattern — no ports, no Docker.
// They verify:
//   1. Correct HTTP status codes
//   2. Correct JSON structure (field names, types)
//   3. JSON Schema conformance against official SignalK schemas
//   4. Spec-mandated behaviour (e.g. 404 on missing path, 501 on snapshot)
//
// To compare against the original SignalK server, see tests/conformance/compare.js.

mod helpers;
use helpers::{
    assert_valid_schema, get, post_json, put_json, test_app, test_app_with_data_dir,
    test_app_with_handler,
};
use serde_json::json;

// ─── GET /signalk — Discovery ────────────────────────────────────────────────

#[tokio::test]
async fn discovery_returns_200() {
    let (status, _) = get(test_app(), "/signalk").await;
    assert_eq!(status, 200, "Discovery endpoint must return HTTP 200");
}

#[tokio::test]
async fn discovery_has_endpoints_field() {
    let (_, body) = get(test_app(), "/signalk").await;
    assert!(
        body["endpoints"].is_object(),
        "Discovery response must have 'endpoints' object, got: {}",
        body
    );
}

#[tokio::test]
async fn discovery_has_v1_endpoint() {
    let (_, body) = get(test_app(), "/signalk").await;
    assert!(
        body["endpoints"]["v1"].is_object(),
        "Discovery must expose 'v1' endpoint, got endpoints: {}",
        body["endpoints"]
    );
}

#[tokio::test]
async fn discovery_v1_has_required_fields() {
    let (_, body) = get(test_app(), "/signalk").await;
    let v1 = &body["endpoints"]["v1"];
    assert!(
        v1["version"].is_string(),
        "v1 endpoint must have 'version' string"
    );
    assert!(
        v1["signalk-http"].is_string(),
        "v1 endpoint must have 'signalk-http' string"
    );
    assert!(
        v1["signalk-ws"].is_string(),
        "v1 endpoint must have 'signalk-ws' string"
    );
}

#[tokio::test]
async fn discovery_version_is_correct() {
    let (_, body) = get(test_app(), "/signalk").await;
    let version = body["endpoints"]["v1"]["version"].as_str().unwrap();
    assert_eq!(version, "1.7.0", "SignalK version must be 1.7.0");
}

#[tokio::test]
async fn discovery_has_server_field() {
    let (_, body) = get(test_app(), "/signalk").await;
    assert!(
        body["server"]["id"].is_string(),
        "Discovery must have server.id"
    );
    assert!(
        body["server"]["version"].is_string(),
        "Discovery must have server.version"
    );
}

// ─── GET /signalk/v1/api — Full model ────────────────────────────────────────

#[tokio::test]
async fn full_model_returns_200() {
    let (status, _) = get(test_app(), "/signalk/v1/api").await;
    assert_eq!(status, 200, "Full model endpoint must return HTTP 200");
}

#[tokio::test]
async fn full_model_has_version() {
    let (_, body) = get(test_app(), "/signalk/v1/api").await;
    assert_eq!(
        body["version"].as_str().unwrap_or(""),
        "1.7.0",
        "Full model must include 'version' field"
    );
}

#[tokio::test]
async fn full_model_has_self_field() {
    let (_, body) = get(test_app(), "/signalk/v1/api").await;
    assert!(
        body["self"].is_string(),
        "Full model must have 'self' string field"
    );
}

#[tokio::test]
async fn full_model_self_is_valid_urn() {
    let (_, body) = get(test_app(), "/signalk/v1/api").await;
    let self_uri = body["self"].as_str().unwrap();
    assert!(
        self_uri.starts_with("urn:mrn:signalk:uuid:"),
        "Self URI must be a SignalK UUID URN, got: {}",
        self_uri
    );
}

#[tokio::test]
async fn full_model_has_vessels_object() {
    let (_, body) = get(test_app(), "/signalk/v1/api").await;
    assert!(
        body["vessels"].is_object(),
        "Full model must have 'vessels' object"
    );
}

#[tokio::test]
async fn full_model_trailing_slash() {
    // Spec requires both /signalk/v1/api and /signalk/v1/api/ to work
    let (status, body) = get(test_app(), "/signalk/v1/api/").await;
    assert_eq!(status, 200);
    assert!(body["version"].is_string());
}

// ─── GET /signalk/v1/api/{path} — Path traversal ─────────────────────────────

#[tokio::test]
async fn path_traversal_vessels_returns_200() {
    let (status, body) = get(test_app(), "/signalk/v1/api/vessels").await;
    assert_eq!(status, 200, "GET /vessels must return 200, got: {}", body);
}

#[tokio::test]
async fn path_traversal_missing_path_returns_404() {
    let (status, _) = get(test_app(), "/signalk/v1/api/does/not/exist").await;
    assert_eq!(status, 404, "Missing path must return 404");
}

#[tokio::test]
async fn path_traversal_vessels_self_returns_200() {
    let (status, _) = get(test_app(), "/signalk/v1/api/vessels/self").await;
    assert_eq!(
        status, 200,
        "GET /vessels/self must return 200 when vessel exists"
    );
}

// ─── GET /signalk/v1/snapshot — History ──────────────────────────────────────

#[tokio::test]
async fn snapshot_returns_501() {
    let (status, body) = get(test_app(), "/signalk/v1/snapshot").await;
    assert_eq!(
        status, 501,
        "Snapshot endpoint must return 501 Not Implemented, got: {} {}",
        status, body
    );
}

// ─── POST /signalk/v1/auth/login — Authentication ────────────────────────────

#[tokio::test]
async fn login_valid_credentials_returns_200() {
    let (status, body) = post_json(
        test_app(),
        "/signalk/v1/auth/login",
        json!({"username": "admin", "password": "anything"}),
    )
    .await;
    // Dev mode: empty password hash accepts any password
    assert_eq!(
        status, 200,
        "Login with valid username must succeed in dev mode, got: {}",
        body
    );
}

#[tokio::test]
async fn login_response_has_token() {
    let (_, body) = post_json(
        test_app(),
        "/signalk/v1/auth/login",
        json!({"username": "admin", "password": "anything"}),
    )
    .await;
    assert!(
        body["token"].is_string(),
        "Login response must contain 'token' string, got: {}",
        body
    );
}

#[tokio::test]
async fn login_response_token_is_jwt() {
    let (_, body) = post_json(
        test_app(),
        "/signalk/v1/auth/login",
        json!({"username": "admin", "password": "x"}),
    )
    .await;
    let token = body["token"].as_str().unwrap_or("");
    // JWT format: three dot-separated base64url segments
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "Token must be a JWT (3 dot-separated parts), got: {}",
        token
    );
}

#[tokio::test]
async fn login_wrong_username_returns_401() {
    let (status, _) = post_json(
        test_app(),
        "/signalk/v1/auth/login",
        json!({"username": "hacker", "password": "anything"}),
    )
    .await;
    assert_eq!(status, 401, "Wrong username must return 401");
}

#[tokio::test]
async fn login_response_has_time_to_live() {
    let (_, body) = post_json(
        test_app(),
        "/signalk/v1/auth/login",
        json!({"username": "admin", "password": "x"}),
    )
    .await;
    // timeToLive is optional per spec but we always include it
    assert!(
        body["timeToLive"].is_number() || body["time_to_live"].is_number(),
        "Login response should include timeToLive, got: {}",
        body
    );
}

// ─── JSON Schema conformance ─────────────────────────────────────────────────

#[test]
fn delta_schema_validates_sample_delta() {
    // Validate a known-good delta against the spec schema.
    // Plain #[test] (not async) to avoid tokio runtime conflicts with jsonschema.
    let delta = json!({
        "context": "vessels.urn:mrn:signalk:uuid:test",
        "updates": [{
            "source": { "label": "ttyUSB0", "type": "NMEA0183", "talker": "GP" },
            "timestamp": "2024-02-26T12:34:56.000Z",
            "values": [
                { "path": "navigation.speedOverGround", "value": 3.85 }
            ]
        }]
    });
    assert_valid_schema("delta", &delta);
}

#[tokio::test]
async fn signalk_schema_validates_full_model_response() {
    let (_, body) = get(test_app(), "/signalk/v1/api").await;
    // Run schema validation on a plain OS thread — jsonschema may create its own
    // runtime internally which would panic if called from within tokio's executor.
    std::thread::spawn(move || {
        assert_valid_schema("signalk", &body);
    })
    .join()
    .unwrap();
}

// ─── HTTP method conformance ──────────────────────────────────────────────────

#[tokio::test]
async fn unknown_route_returns_404() {
    let (status, _) = get(test_app(), "/this/does/not/exist").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn signalk_route_exists() {
    // The root /signalk MUST be accessible — it's the entry point
    let (status, _) = get(test_app(), "/signalk").await;
    assert_ne!(status, 404, "/signalk discovery endpoint must exist");
}

// ─── PUT forwarding ───────────────────────────────────────────────────────────

#[tokio::test]
async fn put_path_no_handler_returns_404() {
    let (status, _) = put_json(
        test_app(),
        "/signalk/v1/api/vessels/self/navigation/speedOverGround",
        json!({"value": 3.5}),
    )
    .await;
    assert_eq!(
        status, 404,
        "PUT with no registered handler must return 404"
    );
}

#[tokio::test]
async fn put_path_bridge_unreachable_returns_503() {
    // Register a handler but point to a non-existent bridge socket.
    let app = test_app_with_handler(
        "steering.autopilot.target.headingTrue",
        "test-plugin",
        "/tmp/signalk-rs-nonexistent-bridge.sock",
    );
    let (status, body) = put_json(
        app,
        "/signalk/v1/api/vessels/self/steering/autopilot/target/headingTrue",
        json!({"value": 2.618}),
    )
    .await;
    assert_eq!(
        status, 503,
        "PUT with unreachable bridge must return 503, body: {}",
        body
    );
}

#[tokio::test]
async fn put_path_wildcard_handler_bridge_unreachable() {
    // Wildcard patterns: "steering.autopilot.target.*" matches the last segment.
    // (The `*` wildcard matches exactly one segment per the matches_pattern implementation.)
    let app = test_app_with_handler(
        "steering.autopilot.target.*",
        "autopilot-plugin",
        "/tmp/signalk-rs-nonexistent-bridge.sock",
    );
    let (status, _) = put_json(
        app,
        "/signalk/v1/api/vessels/self/steering/autopilot/target/headingTrue",
        json!({"value": 2.618}),
    )
    .await;
    assert_eq!(
        status, 503,
        "PUT matching a wildcard handler must attempt forwarding (→ 503 when bridge absent)"
    );
}

// ─── applicationData ────────────────────────────────────────────────────────

#[tokio::test]
async fn app_data_missing_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app_with_data_dir(tmp.path());
    let (status, _) = get(app, "/signalk/v1/applicationData/my-app/1.0.0").await;
    assert_eq!(status, 404, "Missing app data should return 404");
}

#[tokio::test]
async fn app_data_post_and_get() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app_with_data_dir(tmp.path());

    let data = json!({"layout": "grid", "panels": [1, 2, 3]});
    let (status, _) = post_json(
        app.clone(),
        "/signalk/v1/applicationData/my-app/1.0.0",
        data.clone(),
    )
    .await;
    assert_eq!(status, 200, "POST app data should return 200");

    let (status, body) = get(app, "/signalk/v1/applicationData/my-app/1.0.0").await;
    assert_eq!(status, 200, "GET app data should return 200 after POST");
    assert_eq!(
        body, data,
        "GET should return the same data that was POST-ed"
    );
}

#[tokio::test]
async fn app_data_sub_key_get() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app_with_data_dir(tmp.path());

    let data = json!({"settings": {"theme": "dark", "lang": "de"}});
    post_json(
        app.clone(),
        "/signalk/v1/applicationData/my-app/1.0.0",
        data,
    )
    .await;

    let (status, body) = get(
        app,
        "/signalk/v1/applicationData/my-app/1.0.0/settings/theme",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body, "dark");
}

#[tokio::test]
async fn app_data_sub_key_post() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app_with_data_dir(tmp.path());

    // Create initial data
    post_json(
        app.clone(),
        "/signalk/v1/applicationData/my-app/1.0.0",
        json!({"existing": true}),
    )
    .await;

    // Set a sub-key
    let (status, _) = post_json(
        app.clone(),
        "/signalk/v1/applicationData/my-app/1.0.0/settings/theme",
        json!("dark"),
    )
    .await;
    assert_eq!(status, 200);

    // Verify the entire structure
    let (_, body) = get(app, "/signalk/v1/applicationData/my-app/1.0.0").await;
    assert_eq!(body["existing"], true, "Existing data should be preserved");
    assert_eq!(
        body["settings"]["theme"], "dark",
        "New sub-key should exist"
    );
}

#[tokio::test]
async fn app_data_different_versions() {
    let tmp = tempfile::tempdir().unwrap();
    let app = test_app_with_data_dir(tmp.path());

    post_json(
        app.clone(),
        "/signalk/v1/applicationData/my-app/1.0.0",
        json!({"version": "one"}),
    )
    .await;
    post_json(
        app.clone(),
        "/signalk/v1/applicationData/my-app/2.0.0",
        json!({"version": "two"}),
    )
    .await;

    let (_, v1) = get(app.clone(), "/signalk/v1/applicationData/my-app/1.0.0").await;
    let (_, v2) = get(app, "/signalk/v1/applicationData/my-app/2.0.0").await;
    assert_eq!(v1["version"], "one");
    assert_eq!(v2["version"], "two");
}
