mod helpers;

use helpers::{get, test_app, test_app_with_state};

#[tokio::test]
async fn features_returns_200() {
    let app = test_app();
    let (status, body) = get(app, "/signalk/v2/features").await;
    assert_eq!(status, 200);
    assert!(body.get("apis").is_some());
    assert!(body.get("plugins").is_some());
}

#[tokio::test]
async fn features_includes_apis() {
    let app = test_app();
    let (_, body) = get(app, "/signalk/v2/features").await;
    let apis = body["apis"].as_array().unwrap();
    let api_ids: Vec<&str> = apis.iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert!(api_ids.contains(&"resources"), "Should list resources API");
    assert!(api_ids.contains(&"course"), "Should list course API");
    assert!(api_ids.contains(&"autopilot"), "Should list autopilot API");
    assert!(
        api_ids.contains(&"notifications"),
        "Should list notifications API"
    );
    assert!(api_ids.contains(&"history"), "Should list history API");
}

#[tokio::test]
async fn features_includes_plugins() {
    let (app, state) = test_app_with_state();

    // Register a plugin in the registry
    {
        let mut registry = state.plugin_registry.write().await;
        registry.register_tier1(
            "test-plugin",
            "Test Plugin",
            "A test plugin",
            "1.0.0",
            "running",
            true,
        );
    }

    let (status, body) = get(app, "/signalk/v2/features").await;
    assert_eq!(status, 200);

    let plugins = body["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["id"], "test-plugin");
    assert_eq!(plugins[0]["name"], "Test Plugin");
    assert_eq!(plugins[0]["enabled"], true);
}
