mod helpers;

use helpers::{get, test_app, test_app_with_state};
use tower::ServiceExt;

// ─── /skServer/loginStatus ──────────────────────────────────────────────────

#[tokio::test]
async fn login_status_returns_200() {
    let app = test_app();
    let (status, _) = get(app, "/skServer/loginStatus").await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn login_status_has_expected_fields() {
    let app = test_app();
    let (_, body) = get(app, "/skServer/loginStatus").await;
    assert_eq!(body["status"], "notLoggedIn");
    assert_eq!(body["readOnly"], false);
    assert_eq!(body["authenticationRequired"], false);
}

// ─── /skServer/plugins ──────────────────────────────────────────────────────

#[tokio::test]
async fn skserver_plugins_returns_200() {
    let app = test_app();
    let (status, body) = get(app, "/skServer/plugins").await;
    assert_eq!(status, 200);
    assert!(body.is_array());
}

#[tokio::test]
async fn skserver_plugins_lists_registered() {
    let (app, state) = test_app_with_state();

    // Register a plugin in the registry
    state.plugin_registry.write().await.register_tier1(
        "test-plugin",
        "Test Plugin",
        "A test",
        "0.1.0",
        "running",
        true,
    );

    let (status, body) = get(app, "/skServer/plugins").await;
    assert_eq!(status, 200);
    let plugins = body.as_array().unwrap();
    assert!(plugins.iter().any(|p| p["id"] == "test-plugin"));
}

// ─── /skServer/webapps ──────────────────────────────────────────────────────

#[tokio::test]
async fn skserver_webapps_returns_200() {
    let app = test_app();
    let (status, body) = get(app, "/skServer/webapps").await;
    assert_eq!(status, 200);
    assert!(body.is_array());
}

// ─── /skServer/settings ─────────────────────────────────────────────────────

#[tokio::test]
async fn skserver_settings_returns_port() {
    let app = test_app();
    let (status, body) = get(app, "/skServer/settings").await;
    assert_eq!(status, 200);
    assert_eq!(body["port"], 3000);
}

// ─── /skServer/vessel ───────────────────────────────────────────────────────

#[tokio::test]
async fn skserver_vessel_returns_uuid() {
    let app = test_app();
    let (status, body) = get(app, "/skServer/vessel").await;
    assert_eq!(status, 200);
    assert!(
        body["uuid"]
            .as_str()
            .unwrap()
            .starts_with("urn:mrn:signalk:uuid:")
    );
}

// ─── Stub endpoints ─────────────────────────────────────────────────────────

#[tokio::test]
async fn skserver_stubs_return_empty() {
    let app = test_app();

    let (status, body) = get(app.clone(), "/skServer/addons").await;
    assert_eq!(status, 200);
    assert_eq!(body, serde_json::json!([]));

    let (status, body) = get(app.clone(), "/skServer/appstore/available").await;
    assert_eq!(status, 200);
    assert!(body["installing"].is_array());
    assert!(body["available"].is_array());

    let (status, body) = get(app.clone(), "/skServer/security/config").await;
    assert_eq!(status, 200);
    assert_eq!(body, serde_json::json!({}));

    let (status, body) = get(app.clone(), "/skServer/sourcePriorities").await;
    assert_eq!(status, 200);
    assert_eq!(body, serde_json::json!({}));

    let (status, body) = get(app, "/skServer/providers").await;
    assert_eq!(status, 200);
    assert_eq!(body, serde_json::json!([]));
}

// ─── Admin UI compatibility (format-sensitive) ──────────────────────────────

#[tokio::test]
async fn appstore_available_returns_object_with_arrays() {
    let app = test_app();
    let (status, body) = get(app, "/skServer/appstore/available").await;
    assert_eq!(status, 200);
    // Admin UI Redux reducer directly accesses these without null-checks
    assert!(body["available"].is_array());
    assert!(body["installed"].is_array());
    assert!(body["installing"].is_array());
    assert!(body["updates"].is_array());
}

#[tokio::test]
async fn logfiles_returns_200_json() {
    let app = test_app();
    let (status, body) = get(app, "/skServer/logfiles/").await;
    assert_eq!(status, 200);
    assert!(body.is_array());
}

#[tokio::test]
async fn run_discovery_returns_200() {
    let app = test_app();
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/skServer/runDiscovery")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
}

#[tokio::test]
async fn plugins_have_keywords_array() {
    let (app, state) = test_app_with_state();
    state.plugin_registry.write().await.register_tier1(
        "test-kw",
        "Test",
        "desc",
        "0.1.0",
        "running",
        true,
    );

    let (_, body) = get(app, "/skServer/plugins").await;
    let plugin = body.as_array().unwrap().iter().find(|p| p["id"] == "test-kw").unwrap();
    assert!(plugin["keywords"].is_array());
    assert!(!plugin["keywords"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn plugin_config_returns_configuration_field() {
    let (app, state) = test_app_with_state();
    state.plugin_registry.write().await.register_tier1(
        "cfg-test",
        "Cfg Test",
        "desc",
        "0.1.0",
        "running",
        true,
    );

    let (status, body) = get(app, "/skServer/plugins/cfg-test/config").await;
    assert_eq!(status, 200);
    // Admin UI reads response.data.configuration (not "config")
    assert!(body.get("configuration").is_some());
    assert!(body.get("schema").is_some());
}

#[tokio::test]
async fn settings_has_course_api() {
    let app = test_app();
    let (_, body) = get(app, "/skServer/settings").await;
    // Admin UI accesses this.state.courseApi from settings
    assert!(body["courseApi"].is_object());
}

// ─── applicationData with scope ─────────────────────────────────────────────

#[tokio::test]
async fn app_data_invalid_scope_returns_400() {
    let tmp = tempfile::tempdir().unwrap();
    let app = helpers::test_app_with_data_dir(tmp.path());
    let (status, _) = get(app, "/signalk/v1/applicationData/invalid/my-app/1.0.0").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn app_data_user_scope_works() {
    let tmp = tempfile::tempdir().unwrap();
    let app = helpers::test_app_with_data_dir(tmp.path());

    // POST to user scope
    let (status, _) = helpers::post_json(
        app.clone(),
        "/signalk/v1/applicationData/user/my-app/1.0.0",
        serde_json::json!({"theme": "dark"}),
    )
    .await;
    assert_eq!(status, 200);

    // GET from user scope (falls back to global storage)
    let (status, body) = get(app, "/signalk/v1/applicationData/user/my-app/1.0.0").await;
    assert_eq!(status, 200);
    assert_eq!(body["theme"], "dark");
}
