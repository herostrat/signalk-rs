mod helpers;

use axum::{body::Body, http::Request};
use helpers::{get, put_json};
use tower::ServiceExt;

/// Build a test app backed by a real temp directory for course persistence.
fn test_app_with_course() -> (axum::Router, tempfile::TempDir) {
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

const COURSE_BASE: &str = "/signalk/v2/api/vessels/self/navigation/course";

// ─── Basic course operations ─────────────────────────────────────────────────

#[tokio::test]
async fn get_course_empty_returns_200() {
    let (app, _tmp) = test_app_with_course();
    let (status, body) = get(app, COURSE_BASE).await;
    assert_eq!(status, 200);
    // Empty course should be an empty object
    assert_eq!(body, serde_json::json!({}));
}

#[tokio::test]
async fn set_destination_with_position() {
    let (app, _tmp) = test_app_with_course();

    let (status, _) = put_json(
        app.clone(),
        &format!("{COURSE_BASE}/destination"),
        serde_json::json!({
            "position": { "latitude": 49.27, "longitude": -123.19 }
        }),
    )
    .await;
    assert_eq!(status, 200);

    // Verify course state
    let (status, body) = get(app, COURSE_BASE).await;
    assert_eq!(status, 200);
    assert!(body.get("startTime").is_some());
    assert_eq!(body["nextPoint"]["type"], "DESTINATION");
    assert_eq!(body["nextPoint"]["position"]["latitude"], 49.27);
}

#[tokio::test]
async fn set_destination_without_position_or_href() {
    let (app, _tmp) = test_app_with_course();

    let (status, _) = put_json(
        app,
        &format!("{COURSE_BASE}/destination"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn clear_course() {
    let (app, _tmp) = test_app_with_course();

    // Set a destination first
    put_json(
        app.clone(),
        &format!("{COURSE_BASE}/destination"),
        serde_json::json!({
            "position": { "latitude": 49.0, "longitude": -123.0 }
        }),
    )
    .await;

    // Clear
    let response = app
        .clone()
        .oneshot(Request::delete(COURSE_BASE).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);

    // Verify cleared
    let (_, body) = get(app, COURSE_BASE).await;
    assert_eq!(body, serde_json::json!({}));
}

// ─── Active route operations ─────────────────────────────────────────────────

#[tokio::test]
async fn set_active_route_requires_existing_route() {
    let (app, _tmp) = test_app_with_course();

    // Try to set a route that doesn't exist
    let (status, _) = put_json(
        app,
        &format!("{COURSE_BASE}/activeRoute"),
        serde_json::json!({
            "href": "/resources/routes/nonexistent"
        }),
    )
    .await;
    // Should fail because route doesn't exist
    assert_ne!(status, 200);
}

#[tokio::test]
async fn active_route_with_created_route() {
    let (app, _tmp) = test_app_with_course();

    // Create a route with coordinates (GeoJSON LineString format)
    let (_, create_body) = helpers::post_json(
        app.clone(),
        "/signalk/v2/api/resources/routes",
        serde_json::json!({
            "name": "Test Route",
            "feature": {
                "type": "Feature",
                "geometry": {
                    "type": "LineString",
                    "coordinates": [
                        [-123.0, 49.0],
                        [-123.5, 49.5],
                        [-124.0, 50.0]
                    ]
                }
            }
        }),
    )
    .await;
    let route_id = create_body["id"].as_str().unwrap();

    // Set active route
    let (status, _) = put_json(
        app.clone(),
        &format!("{COURSE_BASE}/activeRoute"),
        serde_json::json!({
            "href": format!("/resources/routes/{route_id}")
        }),
    )
    .await;
    assert_eq!(status, 200);

    // Verify course has active route
    let (_, body) = get(app, COURSE_BASE).await;
    assert!(body.get("activeRoute").is_some());
    assert_eq!(body["nextPoint"]["type"], "WAYPOINT");
    // First waypoint: lon=-123.0, lat=49.0
    assert_eq!(body["nextPoint"]["position"]["latitude"], 49.0);
}

#[tokio::test]
async fn advance_next_point() {
    let (app, _tmp) = test_app_with_course();

    // Create route
    let (_, create_body) = helpers::post_json(
        app.clone(),
        "/signalk/v2/api/resources/routes",
        serde_json::json!({
            "name": "Test Route",
            "feature": {
                "type": "Feature",
                "geometry": {
                    "type": "LineString",
                    "coordinates": [
                        [-123.0, 49.0],
                        [-123.5, 49.5],
                        [-124.0, 50.0]
                    ]
                }
            }
        }),
    )
    .await;
    let route_id = create_body["id"].as_str().unwrap();

    // Set active route
    put_json(
        app.clone(),
        &format!("{COURSE_BASE}/activeRoute"),
        serde_json::json!({
            "href": format!("/resources/routes/{route_id}")
        }),
    )
    .await;

    // Advance to next point
    let (status, _) = put_json(
        app.clone(),
        &format!("{COURSE_BASE}/activeRoute/nextPoint"),
        serde_json::json!({"value": 1}),
    )
    .await;
    assert_eq!(status, 200);

    // Verify we're at the second waypoint
    let (_, body) = get(app, COURSE_BASE).await;
    assert_eq!(body["activeRoute"]["pointIndex"], 1);
    assert_eq!(body["nextPoint"]["position"]["latitude"], 49.5);
}

#[tokio::test]
async fn set_point_index() {
    let (app, _tmp) = test_app_with_course();

    // Create route
    let (_, create_body) = helpers::post_json(
        app.clone(),
        "/signalk/v2/api/resources/routes",
        serde_json::json!({
            "name": "Test Route",
            "feature": {
                "type": "Feature",
                "geometry": {
                    "type": "LineString",
                    "coordinates": [
                        [-123.0, 49.0],
                        [-123.5, 49.5],
                        [-124.0, 50.0]
                    ]
                }
            }
        }),
    )
    .await;
    let route_id = create_body["id"].as_str().unwrap();

    // Set active route
    put_json(
        app.clone(),
        &format!("{COURSE_BASE}/activeRoute"),
        serde_json::json!({
            "href": format!("/resources/routes/{route_id}")
        }),
    )
    .await;

    // Jump to last waypoint
    let (status, _) = put_json(
        app.clone(),
        &format!("{COURSE_BASE}/activeRoute/pointIndex"),
        serde_json::json!({"value": 2}),
    )
    .await;
    assert_eq!(status, 200);

    let (_, body) = get(app, COURSE_BASE).await;
    assert_eq!(body["activeRoute"]["pointIndex"], 2);
    assert_eq!(body["nextPoint"]["position"]["latitude"], 50.0);
}

#[tokio::test]
async fn advance_past_end_returns_error() {
    let (app, _tmp) = test_app_with_course();

    // Create route with 2 points
    let (_, create_body) = helpers::post_json(
        app.clone(),
        "/signalk/v2/api/resources/routes",
        serde_json::json!({
            "name": "Short Route",
            "feature": {
                "type": "Feature",
                "geometry": {
                    "type": "LineString",
                    "coordinates": [[-123.0, 49.0], [-124.0, 50.0]]
                }
            }
        }),
    )
    .await;
    let route_id = create_body["id"].as_str().unwrap();

    put_json(
        app.clone(),
        &format!("{COURSE_BASE}/activeRoute"),
        serde_json::json!({
            "href": format!("/resources/routes/{route_id}")
        }),
    )
    .await;

    // Advance past end
    let (status, _) = put_json(
        app,
        &format!("{COURSE_BASE}/activeRoute/nextPoint"),
        serde_json::json!({"value": 5}),
    )
    .await;
    assert_eq!(status, 400);
}

// ─── No active route ─────────────────────────────────────────────────────────

#[tokio::test]
async fn advance_without_active_route_returns_error() {
    let (app, _tmp) = test_app_with_course();

    let (status, _) = put_json(
        app,
        &format!("{COURSE_BASE}/activeRoute/nextPoint"),
        serde_json::json!({"value": 1}),
    )
    .await;
    assert_eq!(status, 400);
}
