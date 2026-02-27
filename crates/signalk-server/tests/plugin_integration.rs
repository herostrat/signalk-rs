//! Plugin integration tests — exercises the full Rust plugin pipeline:
//! PluginManager → RustPluginContext → SignalKStore → REST API.
//!
//! These tests use the **real** PluginManager (not mocks) with the simulator
//! plugin to verify that data flows correctly from plugin → store → HTTP.
#![cfg(feature = "simulator")]

mod helpers;

use helpers::{get, post_json, test_app_with_store};
use signalk_server::plugins::{
    delta_filter::DeltaFilterChain, host::PutHandlerRegistry, manager::PluginManager,
    routes::PluginRouteTable,
};
use signalk_store::store::SignalKStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Create a PluginManager wired to the given store with all required shared state.
fn test_plugin_manager(store: Arc<RwLock<SignalKStore>>) -> PluginManager {
    PluginManager::new(
        store,
        Arc::new(PluginRouteTable::new()),
        Arc::new(PutHandlerRegistry::new()),
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(DeltaFilterChain::new()),
        Arc::new(RwLock::new(signalk_server::webapps::WebappRegistry::new())),
        PathBuf::from("/tmp/signalk-test/config"),
        PathBuf::from("/tmp/signalk-test/data"),
    )
}

#[tokio::test]
async fn simulator_populates_store() {
    let (_, store) = test_app_with_store();
    let mut mgr = test_plugin_manager(store.clone());

    mgr.register(Box::new(signalk_plugin_simulator::SimulatorPlugin::new()));

    // Start with fast interval (50ms)
    let mut configs = HashMap::new();
    configs.insert(
        "simulator".to_string(),
        serde_json::json!({ "update_interval_ms": 50 }),
    );
    mgr.start_all(&configs).await;

    // Wait for a few ticks
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // The store should now contain navigation.position
    let s = store.read().await;
    let pos = s.get_self_path("navigation.position");
    assert!(pos.is_some(), "Expected navigation.position in store");

    let sog = s.get_self_path("navigation.speedOverGround");
    assert!(
        sog.is_some(),
        "Expected navigation.speedOverGround in store"
    );

    drop(s);
    mgr.stop_all().await;
}

#[tokio::test]
async fn simulator_data_visible_via_rest() {
    let (router, store) = test_app_with_store();
    let mut mgr = test_plugin_manager(store.clone());

    mgr.register(Box::new(signalk_plugin_simulator::SimulatorPlugin::new()));

    let mut configs = HashMap::new();
    configs.insert(
        "simulator".to_string(),
        serde_json::json!({ "update_interval_ms": 50 }),
    );
    mgr.start_all(&configs).await;

    // Wait for data to be generated
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Query via REST API
    let (status, body) = get(
        router.clone(),
        "/signalk/v1/api/vessels/self/navigation/speedOverGround",
    )
    .await;
    assert_eq!(status, 200, "Expected 200, body: {body}");

    // The response should contain a "value" field with a number
    let value = body.get("value");
    assert!(value.is_some(), "Expected 'value' field in: {body}");
    assert!(
        value.unwrap().is_number(),
        "Expected numeric value, got: {}",
        value.unwrap()
    );

    mgr.stop_all().await;
}

#[tokio::test]
async fn simulator_emits_magnetic_variation() {
    let (_, store) = test_app_with_store();
    let mut mgr = test_plugin_manager(store.clone());

    mgr.register(Box::new(signalk_plugin_simulator::SimulatorPlugin::new()));

    let mut configs = HashMap::new();
    configs.insert(
        "simulator".to_string(),
        serde_json::json!({
            "update_interval_ms": 50,
            "magnetic_variation_deg": 3.0
        }),
    );
    mgr.start_all(&configs).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let s = store.read().await;
    let mv = s.get_self_path("navigation.magneticVariation");
    assert!(
        mv.is_some(),
        "Expected navigation.magneticVariation in store"
    );

    // Should be ~3.0 degrees in radians ≈ 0.05236
    let value = mv.unwrap().value.as_f64().unwrap();
    assert!(
        (value - 3.0_f64.to_radians()).abs() < 0.01,
        "Expected ~0.052 rad, got {value}"
    );

    drop(s);
    mgr.stop_all().await;
}

#[tokio::test]
async fn simulator_emits_environment_data() {
    let (_, store) = test_app_with_store();
    let mut mgr = test_plugin_manager(store.clone());

    mgr.register(Box::new(signalk_plugin_simulator::SimulatorPlugin::new()));

    let mut configs = HashMap::new();
    configs.insert(
        "simulator".to_string(),
        serde_json::json!({
            "update_interval_ms": 50,
            "enable_environment": true
        }),
    );
    mgr.start_all(&configs).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let s = store.read().await;

    // Check a selection of environment paths
    assert!(
        s.get_self_path("environment.depth.belowTransducer")
            .is_some(),
        "Expected depth.belowTransducer"
    );
    assert!(
        s.get_self_path("environment.outside.temperature").is_some(),
        "Expected outside.temperature"
    );
    assert!(
        s.get_self_path("environment.outside.humidity").is_some(),
        "Expected outside.humidity"
    );
    assert!(
        s.get_self_path("environment.wind.speedApparent").is_some(),
        "Expected wind.speedApparent"
    );

    drop(s);
    mgr.stop_all().await;
}

#[tokio::test]
async fn simulator_stop_halts_data() {
    let (_, store) = test_app_with_store();
    let mut mgr = test_plugin_manager(store.clone());

    mgr.register(Box::new(signalk_plugin_simulator::SimulatorPlugin::new()));

    let mut configs = HashMap::new();
    configs.insert(
        "simulator".to_string(),
        serde_json::json!({ "update_interval_ms": 50 }),
    );
    mgr.start_all(&configs).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Get timestamp before stop
    let ts_before = {
        let s = store.read().await;
        s.get_self_path("navigation.speedOverGround")
            .map(|v| v.timestamp.clone())
    };
    assert!(ts_before.is_some(), "Expected data before stop");

    // Stop
    mgr.stop_all().await;

    // Wait and check that timestamp hasn't changed
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    let ts_after = {
        let s = store.read().await;
        s.get_self_path("navigation.speedOverGround")
            .map(|v| v.timestamp.clone())
    };

    assert_eq!(
        ts_before, ts_after,
        "Timestamps should not change after stop"
    );
}

#[tokio::test]
async fn test_inject_endpoint_applies_delta() {
    let (router, _store) = test_app_with_store();

    let delta = serde_json::json!({
        "context": "vessels.self",
        "updates": [{
            "source": { "label": "test", "type": "test" },
            "values": [
                { "path": "navigation.speedOverGround", "value": 4.25 },
                { "path": "environment.depth.belowTransducer", "value": 15.3 }
            ]
        }]
    });

    // POST to inject endpoint
    let (status, body) = post_json(router.clone(), "/test/inject", delta).await;
    assert_eq!(status, 200, "Expected 200, got {status}: {body}");
    assert_eq!(body["success"], true);

    // Verify data appears via REST
    let (status, body) = get(
        router.clone(),
        "/signalk/v1/api/vessels/self/navigation/speedOverGround",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["value"], 4.25);

    let (status, body) = get(
        router,
        "/signalk/v1/api/vessels/self/environment/depth/belowTransducer",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["value"], 15.3);
}
