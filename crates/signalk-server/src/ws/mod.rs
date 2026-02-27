/// WebSocket streaming handler — /signalk/v1/stream
///
/// Connection lifecycle:
/// 1. Client connects (optional: ?subscribe=self|all|none, ?sendCachedValues=true|false)
/// 2. Server sends HelloMessage
/// 3. Server sends cached values (if sendCachedValues=true)
/// 4. Server streams delta updates matching client subscriptions
/// 5. Client sends SubscribeMessage / UnsubscribeMessage to adjust subscriptions
use axum::{
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use serde::Deserialize;
use signalk_store::subscription::{ActiveSubscription, filter_delta};
use signalk_types::{
    Delta, HelloMessage, InboundMessage, Source, SubscribeMessage, SubscribeMode,
    SubscriptionPolicy, UnsubscribeMessage,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::ServerState;

#[derive(Debug, Deserialize)]
pub struct StreamParams {
    pub subscribe: Option<String>,
    #[serde(rename = "sendCachedValues")]
    pub send_cached_values: Option<bool>,
    /// InstrumentPanel sends sendMeta=all; accepted but not yet implemented.
    #[serde(rename = "sendMeta")]
    pub send_meta: Option<String>,
}

/// Axum route handler — upgrades HTTP to WebSocket.
pub async fn handler(
    ws: WebSocketUpgrade,
    Query(params): Query<StreamParams>,
    State(state): State<Arc<ServerState>>,
) -> Response {
    let subscribe_mode: SubscribeMode = params
        .subscribe
        .as_deref()
        .unwrap_or("self")
        .parse()
        .unwrap_or_default();
    let send_cached = params.send_cached_values.unwrap_or(true);

    ws.on_upgrade(move |socket| handle_socket(socket, state, subscribe_mode, send_cached))
}

async fn handle_socket(
    mut socket: WebSocket,
    state: Arc<ServerState>,
    subscribe_mode: SubscribeMode,
    send_cached_values: bool,
) {
    info!(
        ?subscribe_mode,
        send_cached_values, "WebSocket client connected"
    );

    // 1. Send Hello message
    let store = state.store.read().await;
    let hello = HelloMessage::new(
        signalk_types::SIGNALK_VERSION,
        Some(format!("vessels.{}", store.self_uri)),
    );
    let hello_json = match serde_json::to_string(&hello) {
        Ok(j) => j,
        Err(e) => {
            warn!("Failed to serialize hello: {}", e);
            return;
        }
    };
    if socket.send(Message::Text(hello_json.into())).await.is_err() {
        return;
    }

    // 2. Build initial subscriptions based on connect mode
    let mut subscriptions: HashMap<String, ActiveSubscription> = HashMap::new();
    match subscribe_mode {
        SubscribeMode::Self_ => {
            // Subscribe to all vessels.self data at 1s period
            subscriptions.insert(
                "default-self".to_string(),
                ActiveSubscription::new("vessels.self", "**", 1000, SubscriptionPolicy::Ideal, 0),
            );
        }
        SubscribeMode::All => {
            subscriptions.insert(
                "default-all".to_string(),
                ActiveSubscription::new("**", "**", 1000, SubscriptionPolicy::Ideal, 0),
            );
        }
        SubscribeMode::None => {
            // No default subscriptions — client must subscribe explicitly
        }
    }

    // 3. Optionally send cached values
    if send_cached_values && !subscriptions.is_empty() {
        let self_uri = store.self_uri.clone();
        // Collect all current values as a synthetic delta
        if let Some(vessel) = store.vessel(&self_uri) {
            let values: Vec<_> = vessel
                .values
                .iter()
                .map(|(path, val)| signalk_types::PathValue::new(path.clone(), val.value.clone()))
                .collect();

            if !values.is_empty() {
                let cached_delta = Delta::with_context(
                    format!("vessels.{}", self_uri),
                    vec![signalk_types::Update::new(Source::internal(), values)],
                );
                if let Some(filtered) = filter_delta(&cached_delta, &mut subscriptions)
                    && let Ok(json) = serde_json::to_string(&filtered)
                {
                    let _ = socket.send(Message::Text(json.into())).await;
                }
            }
        }
    }

    // Subscribe to broadcast channel for live updates
    let mut rx = store.subscribe();
    drop(store);

    // 4. Main event loop
    loop {
        tokio::select! {
            // Incoming delta from broadcast channel
            result = rx.recv() => {
                match result {
                    Ok(delta) => {
                        if let Some(filtered) = filter_delta(&delta, &mut subscriptions) {
                            match serde_json::to_string(&filtered) {
                                Ok(json) => {
                                    if socket.send(Message::Text(json.into())).await.is_err() {
                                        debug!("WebSocket client disconnected");
                                        break;
                                    }
                                }
                                Err(e) => warn!("Failed to serialize delta: {}", e),
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WebSocket subscriber lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Incoming message from client
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_client_message(&text, &mut subscriptions);
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        debug!("WebSocket client closed connection");
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = socket.send(Message::Pong(data)).await;
                    }
                    Some(Err(e)) => {
                        warn!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    info!("WebSocket client disconnected");
}

/// Process a client control message (subscribe / unsubscribe).
fn handle_client_message(text: &str, subscriptions: &mut HashMap<String, ActiveSubscription>) {
    // Ignore empty keepalive messages (e.g. InstrumentPanel sends "{}" every 10s)
    let trimmed = text.trim();
    if trimmed == "{}" || trimmed.is_empty() {
        return;
    }

    match serde_json::from_str::<InboundMessage>(text) {
        Ok(InboundMessage::Subscribe(msg)) => {
            apply_subscriptions(&msg, subscriptions);
        }
        Ok(InboundMessage::Unsubscribe(msg)) => {
            apply_unsubscribe(&msg, subscriptions);
        }
        Err(e) => {
            warn!("Failed to parse WebSocket message: {} — {}", e, text);
        }
    }
}

fn apply_subscriptions(
    msg: &SubscribeMessage,
    subscriptions: &mut HashMap<String, ActiveSubscription>,
) {
    for sub in &msg.subscribe {
        let key = format!("{}:{}", msg.context, sub.path);
        let period = sub.period.unwrap_or(1000);
        let policy = sub.policy.unwrap_or_default();
        let min_period = sub.min_period.unwrap_or(0);

        subscriptions.insert(
            key,
            ActiveSubscription::new(
                msg.context.clone(),
                sub.path.clone(),
                period,
                policy,
                min_period,
            ),
        );
        debug!(path = %sub.path, "Client subscribed");
    }
}

fn apply_unsubscribe(
    msg: &UnsubscribeMessage,
    subscriptions: &mut HashMap<String, ActiveSubscription>,
) {
    for spec in &msg.unsubscribe {
        if spec.path == "*" {
            // Unsubscribe from all in this context
            subscriptions.retain(|k, _| !k.starts_with(&msg.context));
            debug!(context = %msg.context, "Client unsubscribed from all");
        } else {
            let key = format!("{}:{}", msg.context, spec.path);
            subscriptions.remove(&key);
            debug!(path = %spec.path, "Client unsubscribed");
        }
    }
}
