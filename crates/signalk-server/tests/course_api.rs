mod helpers;

use axum::{body::Body, http::Request};
use helpers::{get, put_json};
use std::sync::Arc;
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
    assert_eq!(body["nextPoint"]["type"], "destination");
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
    assert_eq!(body["nextPoint"]["type"], "waypoint");
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

// ─── api_only semantics ───────────────────────────────────────────────────────

/// Build a test app with course support AND return the Arc<ServerState> for direct
/// access (e.g. to start the NMEA listener and inject deltas).
fn test_app_course_state() -> (
    axum::Router,
    Arc<signalk_server::ServerState>,
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().unwrap();
    let config = signalk_server::config::ServerConfig {
        data_dir: tmp.path().to_string_lossy().to_string(),
        ..signalk_server::config::ServerConfig::default()
    };
    let (store, _rx) = signalk_store::store::SignalKStore::new(config.vessel.uuid.clone());
    let state = signalk_server::ServerState::new(config, store);
    let router = signalk_server::build_router(state.clone(), &[]);
    (router, state, tmp)
}

/// Inject an NMEA-style delta with a nextPoint position into the store.
fn nmea_course_delta(lat: f64, lon: f64) -> signalk_types::Delta {
    signalk_types::Delta::self_vessel(vec![signalk_types::Update::new(
        signalk_types::Source::nmea0183("gps-chartplotter", "GP"),
        vec![signalk_types::PathValue::new(
            "navigation.courseGreatCircle.nextPoint.position",
            serde_json::json!({ "latitude": lat, "longitude": lon }),
        )],
    )])
}

#[tokio::test]
async fn api_only_false_nmea_sets_course() {
    let (app, state, _tmp) = test_app_course_state();

    // Subscribe before injecting delta
    let rx = state.store.read().await.subscribe();
    state.course_manager.clone().start_nmea_listener(rx).await;

    // Inject NMEA delta
    let delta = nmea_course_delta(54.0, 10.0);
    state.store.write().await.apply_delta(delta);

    // Give the listener time to process
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (status, body) = get(app, COURSE_BASE).await;
    assert_eq!(status, 200);
    assert_eq!(body["nextPoint"]["position"]["latitude"], 54.0);
    assert_eq!(body["nextPoint"]["position"]["longitude"], 10.0);
}

#[tokio::test]
async fn api_only_true_nmea_ignored() {
    let (app, state, _tmp) = test_app_course_state();

    // Enable api_only before starting the listener
    state.course_manager.enable_api_only().await;

    let rx = state.store.read().await.subscribe();
    state.course_manager.clone().start_nmea_listener(rx).await;

    // Inject NMEA delta — should be silently ignored
    let delta = nmea_course_delta(54.0, 10.0);
    state.store.write().await.apply_delta(delta);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Course should still be empty
    let (status, body) = get(app, COURSE_BASE).await;
    assert_eq!(status, 200);
    assert_eq!(body, serde_json::json!({}));
}

#[tokio::test]
async fn enable_api_only_clears_nmea_course() {
    let (app, state, _tmp) = test_app_course_state();

    let rx = state.store.read().await.subscribe();
    state.course_manager.clone().start_nmea_listener(rx).await;

    // Set course via NMEA
    let delta = nmea_course_delta(54.0, 10.0);
    state.store.write().await.apply_delta(delta);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify course is set
    let (_, body) = get(app.clone(), COURSE_BASE).await;
    assert_eq!(body["nextPoint"]["position"]["latitude"], 54.0);

    // Enable api_only — should auto-clear the NMEA-sourced course
    state.course_manager.enable_api_only().await;

    let (status, body) = get(app, COURSE_BASE).await;
    assert_eq!(status, 200);
    assert_eq!(body, serde_json::json!({}));
}

#[tokio::test]
async fn rest_api_always_works_regardless_of_api_only() {
    let (app, state, _tmp) = test_app_course_state();

    // Enable api_only
    state.course_manager.enable_api_only().await;

    // REST PUT destination must still succeed with api_only=true
    let (status, _) = put_json(
        app.clone(),
        &format!("{COURSE_BASE}/destination"),
        serde_json::json!({
            "position": { "latitude": 55.0, "longitude": 12.0 }
        }),
    )
    .await;
    assert_eq!(status, 200);

    let (_, body) = get(app, COURSE_BASE).await;
    assert_eq!(body["nextPoint"]["position"]["latitude"], 55.0);
}
