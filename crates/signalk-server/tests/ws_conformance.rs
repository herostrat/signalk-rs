// WebSocket protocol conformance tests.
//
// These tests spin up a real TCP listener (port 0) and connect with a
// tokio-tungstenite client — the only reliable way to test WS upgrades
// without a separate integration harness.
//
// Each test creates a fresh server so port assignments never collide.

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use signalk_server::{ServerState, build_router, config::ServerConfig};
use signalk_store::store::SignalKStore;
use signalk_types::{Delta, PathValue, Source, Update};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message as WsMsg;

// ── Test server helpers ───────────────────────────────────────────────────────

/// Bind a random port, spawn axum, return the base WS URL and the store Arc.
async fn spawn_ws_server() -> (String, Arc<RwLock<SignalKStore>>) {
    let config = ServerConfig::default();
    let (store, _rx) = SignalKStore::new(config.vessel.uuid.clone());
    let state = ServerState::new(config, store.clone());
    let router = build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // Yield so the spawned accept loop can start
    tokio::task::yield_now().await;

    (format!("ws://127.0.0.1:{port}"), store)
}

async fn connect(
    base_url: &str,
    params: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("{base_url}/signalk/v1/stream{params}");
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws
}

async fn recv_text(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> serde_json::Value {
    let msg = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .expect("timeout waiting for WS message")
        .unwrap()
        .unwrap();
    match msg {
        WsMsg::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("Expected text frame, got: {:?}", other),
    }
}

async fn send_json(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    value: serde_json::Value,
) {
    ws.send(WsMsg::Text(serde_json::to_string(&value).unwrap().into()))
        .await
        .unwrap();
}

fn gps_delta() -> Delta {
    Delta::self_vessel(vec![Update::new(
        Source::nmea0183("ttyUSB0", "GP"),
        vec![PathValue::new("navigation.speedOverGround", json!(3.85))],
    )])
}

// ── Hello message ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn ws_hello_has_required_fields() {
    let (base, _store) = spawn_ws_server().await;
    let mut ws = connect(&base, "?subscribe=none").await;

    let hello = recv_text(&mut ws).await;

    assert_eq!(
        hello["version"].as_str().unwrap_or(""),
        "1.7.0",
        "Hello must include SignalK version 1.7.0, got: {}",
        hello
    );
    assert!(hello["self"].is_string(), "Hello must include self URI");
    assert!(hello["roles"].is_array(), "Hello must include roles array");
    assert!(
        hello["timestamp"].is_string(),
        "Hello must include ISO timestamp"
    );
}

#[tokio::test]
async fn ws_hello_server_name() {
    let (base, _store) = spawn_ws_server().await;
    let mut ws = connect(&base, "?subscribe=none").await;
    let hello = recv_text(&mut ws).await;
    assert_eq!(
        hello["name"].as_str(),
        Some("signalk-rs"),
        "Hello must identify server as signalk-rs"
    );
}

#[tokio::test]
async fn ws_hello_roles_contain_master() {
    let (base, _store) = spawn_ws_server().await;
    let mut ws = connect(&base, "?subscribe=none").await;
    let hello = recv_text(&mut ws).await;
    let roles: Vec<&str> = hello["roles"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        roles.contains(&"master"),
        "Server must declare 'master' role, got: {:?}",
        roles
    );
}

// ── subscribe=none: no data after hello ───────────────────────────────────────

#[tokio::test]
async fn ws_subscribe_none_no_cached_values() {
    let (base, _store) = spawn_ws_server().await;
    let mut ws = connect(&base, "?subscribe=none").await;

    // Receive hello
    recv_text(&mut ws).await;

    // No second message should arrive within 200 ms
    let next = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
    assert!(
        next.is_err(),
        "subscribe=none must not send cached values after hello"
    );
}

// ── subscribe=self: cached values sent on connect ─────────────────────────────

#[tokio::test]
async fn ws_subscribe_self_sends_cached_values() {
    let (base, store) = spawn_ws_server().await;

    // Seed the store before the client connects
    store.write().await.apply_delta(gps_delta());

    let mut ws = connect(&base, "?subscribe=self").await;

    // Hello
    recv_text(&mut ws).await;

    // Cached delta should follow immediately
    let cached = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout waiting for cached delta")
        .unwrap()
        .unwrap();
    let cached: serde_json::Value = match cached {
        WsMsg::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text, got {:?}", other),
    };
    let empty = vec![];
    let paths: Vec<&str> = cached["updates"][0]["values"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|v| v["path"].as_str())
        .collect();
    assert!(
        paths.contains(&"navigation.speedOverGround"),
        "Cached delta must include navigation.speedOverGround, got paths: {:?}",
        paths
    );
}

// ── Live delta fanout ─────────────────────────────────────────────────────────

#[tokio::test]
async fn ws_live_delta_after_subscribe() {
    let (base, store) = spawn_ws_server().await;
    let mut ws = connect(&base, "?subscribe=none").await;

    // Hello
    recv_text(&mut ws).await;

    // Subscribe to navigation paths
    send_json(
        &mut ws,
        json!({
            "context": "vessels.self",
            "subscribe": [{"path": "navigation.*"}]
        }),
    )
    .await;

    // Give the server a moment to register the subscription
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Inject delta into store
    store.write().await.apply_delta(gps_delta());

    // Client should receive the delta
    let delta = recv_text(&mut ws).await;
    assert!(
        delta["updates"].is_array(),
        "Live delta must have 'updates' array"
    );
    let empty = vec![];
    let paths: Vec<&str> = delta["updates"][0]["values"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|v| v["path"].as_str())
        .collect();
    assert!(
        paths.contains(&"navigation.speedOverGround"),
        "Must receive navigation.speedOverGround, got: {:?}",
        paths
    );
}

#[tokio::test]
async fn ws_unsubscribed_path_not_delivered() {
    let (base, store) = spawn_ws_server().await;
    let mut ws = connect(&base, "?subscribe=none").await;

    // Hello
    recv_text(&mut ws).await;

    // Subscribe to propulsion.* only
    send_json(
        &mut ws,
        json!({
            "context": "vessels.self",
            "subscribe": [{"path": "propulsion.*"}]
        }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Inject a GPS (navigation.*) delta — should NOT match
    store.write().await.apply_delta(gps_delta());

    let result = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
    assert!(
        result.is_err(),
        "Unsubscribed path navigation.* must not be delivered to propulsion.* subscriber"
    );
}

// ── Multiple clients receive the same delta ───────────────────────────────────

#[tokio::test]
async fn ws_multiple_clients_receive_same_delta() {
    let (base, store) = spawn_ws_server().await;

    let mut ws1 = connect(&base, "?subscribe=none").await;
    let mut ws2 = connect(&base, "?subscribe=none").await;

    // Both receive hello
    recv_text(&mut ws1).await;
    recv_text(&mut ws2).await;

    // Both subscribe to navigation.*
    let sub = json!({
        "context": "vessels.self",
        "subscribe": [{"path": "navigation.*"}]
    });
    send_json(&mut ws1, sub.clone()).await;
    send_json(&mut ws2, sub).await;
    tokio::time::sleep(Duration::from_millis(30)).await;

    // Inject one delta
    store.write().await.apply_delta(gps_delta());

    // Both must receive it
    let d1 = recv_text(&mut ws1).await;
    let d2 = recv_text(&mut ws2).await;

    assert!(d1["updates"].is_array(), "Client 1 must receive delta");
    assert!(d2["updates"].is_array(), "Client 2 must receive delta");
}

// ── Ping / Pong ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn ws_server_responds_to_ping() {
    let (base, _store) = spawn_ws_server().await;
    let mut ws = connect(&base, "?subscribe=none").await;

    // Hello
    recv_text(&mut ws).await;

    // Send ping
    ws.send(WsMsg::Ping(b"heartbeat".to_vec().into()))
        .await
        .unwrap();

    // Next message must be a Pong
    let pong = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout waiting for pong")
        .unwrap()
        .unwrap();
    assert!(
        matches!(pong, WsMsg::Pong(_)),
        "Server must respond to Ping with Pong, got: {:?}",
        pong
    );
}
