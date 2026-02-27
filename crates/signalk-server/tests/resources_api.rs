mod helpers;

use helpers::{get, post_json, put_json};

/// Build a test app backed by a real temp directory for resource persistence.
fn test_app_with_resources() -> (axum::Router, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let config = signalk_server::config::ServerConfig {
        data_dir: tmp.path().to_string_lossy().to_string(),
        ..signalk_server::config::ServerConfig::default()
    };
    let (store, _rx) = signalk_store::store::SignalKStore::new(config.vessel.uuid.clone());
    let state = signalk_server::ServerState::new(config, store);
    let router = signalk_server::build_router(state, &[]);
    (router, tmp)
}

/// Helper: create a resource and return (router, id, tmp_dir).
/// The router is cloned from a shared state so it can be reused.
async fn create_waypoint(app: axum::Router) -> (u16, serde_json::Value) {
    post_json(
        app,
        "/signalk/v2/api/resources/waypoints",
        serde_json::json!({
            "name": "Test Waypoint",
            "position": { "latitude": 49.27, "longitude": -123.19 }
        }),
    )
    .await
}

// ─── Basic CRUD ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_waypoint_returns_200_with_id() {
    let (app, _tmp) = test_app_with_resources();
    let (status, body) = create_waypoint(app).await;
    assert_eq!(status, 200);
    assert_eq!(body["state"], "COMPLETED");
    assert_eq!(body["statusCode"], 200);
    assert!(
        body["id"].as_str().unwrap().len() > 10,
        "Expected UUID-format ID"
    );
}

#[tokio::test]
async fn get_created_waypoint() {
    let (app, _tmp) = test_app_with_resources();

    // Create
    let (_, create_body) = create_waypoint(app.clone()).await;
    let id = create_body["id"].as_str().unwrap();

    // Get
    let (status, body) = get(app, &format!("/signalk/v2/api/resources/waypoints/{id}")).await;
    assert_eq!(status, 200);
    assert_eq!(body["name"], "Test Waypoint");
}

#[tokio::test]
async fn list_waypoints_contains_created() {
    let (app, _tmp) = test_app_with_resources();

    let (_, create_body) = create_waypoint(app.clone()).await;
    let id = create_body["id"].as_str().unwrap();

    let (status, body) = get(app, "/signalk/v2/api/resources/waypoints").await;
    assert_eq!(status, 200);
    assert!(body.get(id).is_some(), "Expected created waypoint in list");
}

#[tokio::test]
async fn update_waypoint() {
    let (app, _tmp) = test_app_with_resources();

    let (_, create_body) = create_waypoint(app.clone()).await;
    let id = create_body["id"].as_str().unwrap();

    let (status, _) = put_json(
        app.clone(),
        &format!("/signalk/v2/api/resources/waypoints/{id}"),
        serde_json::json!({"name": "Updated WP", "position": {"latitude": 50.0, "longitude": -124.0}}),
    )
    .await;
    assert_eq!(status, 200);

    let (_, body) = get(app, &format!("/signalk/v2/api/resources/waypoints/{id}")).await;
    assert_eq!(body["name"], "Updated WP");
}

#[tokio::test]
async fn delete_waypoint() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let (app, _tmp) = test_app_with_resources();

    let (_, create_body) = create_waypoint(app.clone()).await;
    let id = create_body["id"].as_str().unwrap();

    // Delete
    let response = app
        .clone()
        .oneshot(
            Request::delete(format!("/signalk/v2/api/resources/waypoints/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);

    // Verify gone
    let (status, _) = get(app, &format!("/signalk/v2/api/resources/waypoints/{id}")).await;
    assert_eq!(status, 404);
}

// ─── Error cases ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_nonexistent_returns_404() {
    let (app, _tmp) = test_app_with_resources();
    let (status, _) = get(app, "/signalk/v2/api/resources/waypoints/no-such-id").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn unknown_resource_type_returns_404() {
    let (app, _tmp) = test_app_with_resources();
    let (status, _) = get(app, "/signalk/v2/api/resources/foobar").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn delete_nonexistent_returns_404() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let (app, _tmp) = test_app_with_resources();
    let response = app
        .oneshot(
            Request::delete("/signalk/v2/api/resources/waypoints/no-such-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn update_nonexistent_returns_404() {
    let (app, _tmp) = test_app_with_resources();
    let (status, _) = put_json(
        app,
        "/signalk/v2/api/resources/waypoints/no-such-id",
        serde_json::json!({"name": "test"}),
    )
    .await;
    assert_eq!(status, 404);
}

// ─── All 5 resource types ────────────────────────────────────────────────────

#[tokio::test]
async fn all_resource_types_accept_crud() {
    let types = ["waypoints", "routes", "notes", "regions", "charts"];

    for resource_type in types {
        let (app, _tmp) = test_app_with_resources();

        // Create
        let (status, body) = post_json(
            app.clone(),
            &format!("/signalk/v2/api/resources/{resource_type}"),
            serde_json::json!({"name": format!("Test {resource_type}")}),
        )
        .await;
        assert_eq!(status, 200, "Create failed for {resource_type}");

        let id = body["id"].as_str().unwrap();

        // Get
        let (status, _) = get(
            app.clone(),
            &format!("/signalk/v2/api/resources/{resource_type}/{id}"),
        )
        .await;
        assert_eq!(status, 200, "Get failed for {resource_type}");

        // List
        let (status, list) = get(app, &format!("/signalk/v2/api/resources/{resource_type}")).await;
        assert_eq!(status, 200, "List failed for {resource_type}");
        assert!(
            !list.as_object().unwrap().is_empty(),
            "List empty for {resource_type}"
        );
    }
}

// ─── Query parameter: limit ──────────────────────────────────────────────────

#[tokio::test]
async fn list_with_limit() {
    let (app, _tmp) = test_app_with_resources();

    // Create 3 waypoints
    for i in 0..3 {
        post_json(
            app.clone(),
            "/signalk/v2/api/resources/waypoints",
            serde_json::json!({"name": format!("WP {i}")}),
        )
        .await;
    }

    let (status, body) = get(app, "/signalk/v2/api/resources/waypoints?limit=2").await;
    assert_eq!(status, 200);
    assert_eq!(
        body.as_object().unwrap().len(),
        2,
        "Expected limit to restrict to 2 results"
    );
}

// ─── Path traversal protection ──────────────────────────────────────────────

#[tokio::test]
async fn path_traversal_in_type_rejected() {
    let (app, _tmp) = test_app_with_resources();
    // axum will decode this, but our handler validates
    let (status, _) = get(app, "/signalk/v2/api/resources/..%2Fetc").await;
    assert_eq!(status, 404, "Path traversal in type should be rejected");
}
