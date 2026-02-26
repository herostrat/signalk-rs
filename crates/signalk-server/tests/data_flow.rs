// Data-flow conformance tests: inject delta → store → verify REST output.
//
// These tests exercise the full pipeline:
//   apply_delta() → SignalKStore → full_model() → Serialize → HTTP response → path traversal
//
// Each test creates a fresh app with a seeded store and makes HTTP requests.

mod helpers;
use helpers::{get, test_app_with_store};
use serde_json::json;
use signalk_types::{Delta, PathValue, Source, Update};

// ── Shared test fixtures ──────────────────────────────────────────────────────

fn gps_delta() -> Delta {
    Delta::self_vessel(vec![Update::new(
        Source::nmea0183("ttyUSB0", "GP"),
        vec![
            PathValue::new("navigation.speedOverGround", json!(3.85)),
            PathValue::new("navigation.courseOverGroundTrue", json!(2.971)),
        ],
    )])
}

fn depth_delta() -> Delta {
    Delta::self_vessel(vec![Update::new(
        Source::nmea0183("ttyUSB0", "SD"),
        vec![PathValue::new("environment.depth.belowKeel", json!(12.5))],
    )])
}

/// Returns an app pre-seeded with GPS + depth data, and the self vessel URI.
async fn seeded_app() -> (axum::Router, String) {
    let (app, store) = test_app_with_store();
    let self_uri = store.read().await.self_uri.clone();
    {
        let mut s = store.write().await;
        s.apply_delta(gps_delta());
        s.apply_delta(depth_delta());
    }
    (app, self_uri)
}

// ── Full model includes injected telemetry ────────────────────────────────────

#[tokio::test]
async fn full_model_includes_nested_telemetry() {
    let (app, self_uri) = seeded_app().await;
    let (status, body) = get(app, "/signalk/v1/api").await;

    assert_eq!(status, 200);
    let vessel = &body["vessels"][&self_uri];
    assert_eq!(
        vessel["navigation"]["speedOverGround"]["value"],
        3.85,
        "speedOverGround must appear nested in full model, got vessel: {}",
        serde_json::to_string_pretty(vessel).unwrap()
    );
    assert_eq!(vessel["environment"]["depth"]["belowKeel"]["value"], 12.5);
}

#[tokio::test]
async fn full_model_no_flat_dot_keys() {
    let (app, self_uri) = seeded_app().await;
    let (_, body) = get(app, "/signalk/v1/api").await;

    let vessel = &body["vessels"][&self_uri];
    // Flat dot-notation keys must NOT be top-level vessel fields
    assert!(
        vessel.get("navigation.speedOverGround").is_none(),
        "Flat key 'navigation.speedOverGround' must not appear at vessel top level"
    );
    assert!(
        vessel.get("environment.depth.belowKeel").is_none(),
        "Flat key 'environment.depth.belowKeel' must not appear at vessel top level"
    );
}

#[tokio::test]
async fn full_model_sources_registry_populated() {
    let (app, _) = seeded_app().await;
    let (_, body) = get(app, "/signalk/v1/api").await;

    assert!(
        body["sources"].is_object() && !body["sources"].as_object().unwrap().is_empty(),
        "sources registry must be populated after delta injection, got: {}",
        body["sources"]
    );
}

#[tokio::test]
async fn full_model_source_ref_format() {
    let (app, self_uri) = seeded_app().await;
    let (_, body) = get(app, "/signalk/v1/api").await;

    let source_ref = body["vessels"][&self_uri]["navigation"]["speedOverGround"]["$source"]
        .as_str()
        .unwrap_or("");
    // Source refs follow "{label}.{talker}" convention for NMEA0183
    assert_eq!(
        source_ref, "ttyUSB0.GP",
        "$source must be 'ttyUSB0.GP' for this delta"
    );
}

// ── Leaf path traversal ───────────────────────────────────────────────────────

#[tokio::test]
async fn path_traversal_leaf_value_and_source() {
    let (app, _) = seeded_app().await;
    let (status, body) = get(
        app,
        "/signalk/v1/api/vessels/self/navigation/speedOverGround",
    )
    .await;

    assert_eq!(status, 200, "Leaf path must return 200, got: {}", body);
    assert_eq!(body["value"], 3.85, "Value must match injected SOG");
    assert_eq!(
        body["$source"], "ttyUSB0.GP",
        "$source must be the NMEA talker ref"
    );
    assert!(body["timestamp"].is_string(), "Leaf must include timestamp");
}

#[tokio::test]
async fn path_traversal_second_leaf() {
    let (app, _) = seeded_app().await;
    let (status, body) = get(
        app,
        "/signalk/v1/api/vessels/self/navigation/courseOverGroundTrue",
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["value"], 2.971);
}

// ── Intermediate node traversal ───────────────────────────────────────────────

#[tokio::test]
async fn path_traversal_intermediate_node_contains_children() {
    let (app, _) = seeded_app().await;
    let (status, body) = get(app, "/signalk/v1/api/vessels/self/navigation").await;

    assert_eq!(
        status, 200,
        "Intermediate path must return 200, got: {}",
        body
    );
    assert!(
        body["speedOverGround"].is_object(),
        "navigation node must contain speedOverGround, got: {}",
        body
    );
    assert!(
        body["courseOverGroundTrue"].is_object(),
        "navigation node must contain courseOverGroundTrue, got: {}",
        body
    );
}

#[tokio::test]
async fn path_traversal_three_levels_deep() {
    let (app, _) = seeded_app().await;
    let (status, body) = get(
        app,
        "/signalk/v1/api/vessels/self/environment/depth/belowKeel",
    )
    .await;

    assert_eq!(
        status, 200,
        "3-level deep path must return 200, got: {}",
        body
    );
    assert_eq!(body["value"], 12.5);
}

// ── 404 on missing paths ──────────────────────────────────────────────────────

#[tokio::test]
async fn path_traversal_missing_child_returns_404() {
    let (app, _) = seeded_app().await;
    // The parent "navigation" exists, but "headingTrue" was never injected
    let (status, _) = get(app, "/signalk/v1/api/vessels/self/navigation/headingTrue").await;
    assert_eq!(
        status, 404,
        "Non-existent child path must return 404 even when parent exists"
    );
}

#[tokio::test]
async fn path_traversal_missing_top_level_returns_404() {
    let (app, _) = seeded_app().await;
    let (status, _) = get(app, "/signalk/v1/api/vessels/self/propulsion").await;
    assert_eq!(status, 404, "Missing top-level namespace must return 404");
}

// ── Value updates ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn updated_value_reflected_in_rest() {
    let (app, store) = test_app_with_store();

    // First delta
    store.write().await.apply_delta(gps_delta());
    let (_, body1) = get(
        app.clone(),
        "/signalk/v1/api/vessels/self/navigation/speedOverGround",
    )
    .await;
    assert_eq!(body1["value"], 3.85);

    // Updated delta with new SOG
    store
        .write()
        .await
        .apply_delta(Delta::self_vessel(vec![Update::new(
            Source::nmea0183("ttyUSB0", "GP"),
            vec![PathValue::new("navigation.speedOverGround", json!(5.12))],
        )]));
    let (_, body2) = get(
        app,
        "/signalk/v1/api/vessels/self/navigation/speedOverGround",
    )
    .await;
    assert_eq!(body2["value"], 5.12, "Store must reflect the latest value");
}
