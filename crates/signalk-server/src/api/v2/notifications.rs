/// Notifications API handlers.
///
/// | Method | Route | Handler |
/// |--------|-------|---------|
/// | GET | `/signalk/v2/api/notifications` | `list_notifications` |
/// | POST | `/signalk/v2/api/notifications/{id}/silence` | `silence` |
/// | POST | `/signalk/v2/api/notifications/{id}/acknowledge` | `acknowledge` |
///
/// The `{id}` path segment maps to the notification path after `notifications.`
/// in the SignalK store. For example, `navigation.anchor` maps to the store
/// path `notifications.navigation.anchor`.
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use signalk_types::notification::{
    Notification, NotificationMethod, NotificationState, NotificationStatus,
};
use signalk_types::{Delta, PathValue, Source, Update};
use std::sync::Arc;

use crate::ServerState;

/// Validate that a notification ID is a safe dot-path (no traversal sequences).
fn validate_notification_id(id: &str) -> Result<(), (StatusCode, String)> {
    if id.contains("..") || id.contains('/') || id.starts_with('.') || id.ends_with('.') {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid notification ID".to_string(),
        ));
    }
    Ok(())
}

/// `GET /signalk/v2/api/notifications`
///
/// Returns all active notifications as a JSON object keyed by path (without the
/// `notifications.` prefix). Values are already enriched with `id` and `status`
/// by the NotificationManager delta filter.
pub async fn list_notifications(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let store = state.store.read().await;
    let notifications = store.notifications();
    let mut result = serde_json::Map::new();
    for (path, sv) in notifications {
        // Strip the "notifications." prefix for the response key
        let key = path
            .strip_prefix("notifications.")
            .unwrap_or(path)
            .to_string();
        result.insert(key, sv.value.clone());
    }
    Json(serde_json::Value::Object(result)).into_response()
}

/// `POST /signalk/v2/api/notifications/{notification_id}/silence`
///
/// Removes `Sound` from the notification's method array and sets `status.silenced = true`.
/// Emergency-level notifications cannot be silenced.
pub async fn silence(
    State(state): State<Arc<ServerState>>,
    Path(notification_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = validate_notification_id(&notification_id) {
        return e.into_response();
    }
    let store_path = format!("notifications.{notification_id}");
    let response = mutate_notification(&state, &store_path, |n| {
        if n.state == NotificationState::Emergency {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "Emergency notifications cannot be silenced".to_string(),
            ));
        }
        n.method.retain(|m| *m != NotificationMethod::Sound);
        let status = n.status.get_or_insert_with(NotificationStatus::default);
        status.silenced = Some(true);
        status.can_silence = Some(false); // already silenced
        Ok(())
    })
    .await;

    // Sync internal manager state on success
    if response.status().is_success() {
        state.notification_manager.silence(&store_path);
    }
    response
}

/// `POST /signalk/v2/api/notifications/{notification_id}/acknowledge`
///
/// Removes `Sound` from the method array and sets `status.acknowledged = true`.
/// For Emergency notifications, only `Sound` is removed (not `Visual`).
/// For other notifications, both `Sound` and `Visual` are removed.
pub async fn acknowledge(
    State(state): State<Arc<ServerState>>,
    Path(notification_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = validate_notification_id(&notification_id) {
        return e.into_response();
    }
    let store_path = format!("notifications.{notification_id}");
    let response = mutate_notification(&state, &store_path, |n| {
        if n.state == NotificationState::Emergency {
            // Emergency: only remove sound, keep visual
            n.method.retain(|m| *m != NotificationMethod::Sound);
        } else {
            n.method
                .retain(|m| !matches!(m, NotificationMethod::Sound | NotificationMethod::Visual));
        }
        let status = n.status.get_or_insert_with(NotificationStatus::default);
        status.acknowledged = Some(true);
        status.can_acknowledge = Some(false); // already acknowledged
        Ok(())
    })
    .await;

    // Sync internal manager state on success
    if response.status().is_success() {
        state.notification_manager.acknowledge(&store_path);
    }
    response
}

/// Read a notification from the store, apply `mutate`, write it back as a delta.
///
/// Preserves the `id` from the NotificationManager so the stored value remains
/// consistent with the manager's tracking.
async fn mutate_notification<F>(
    state: &ServerState,
    path: &str,
    mutate: F,
) -> axum::response::Response
where
    F: FnOnce(&mut Notification) -> Result<(), (StatusCode, String)>,
{
    // Read from store
    let raw_value = {
        let store = state.store.read().await;
        match store.get_self_path(path) {
            Some(sv) => sv.value.clone(),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    format!("Notification not found: {path}"),
                )
                    .into_response();
            }
        }
    };

    // Deserialize
    let mut notification: Notification = match serde_json::from_value(raw_value) {
        Ok(n) => n,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to parse notification: {e}"),
            )
                .into_response();
        }
    };

    // Preserve id from manager if not already set
    if notification.id.is_none()
        && let Some((id, _, _, _)) = state.notification_manager.get_entry_data(path)
    {
        notification.id = Some(id);
    }

    // Apply mutation
    if let Err((status, msg)) = mutate(&mut notification) {
        return (status, msg).into_response();
    }

    // Serialize back — failure here would silently corrupt the store, so treat as 500.
    let serialized = match serde_json::to_value(&notification) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize notification: {e}"),
            )
                .into_response();
        }
    };

    // Write back as delta — bypasses DeltaFilterChain (direct store write),
    // so the id + status must already be correct in the serialized value.
    let delta = Delta::self_vessel(vec![Update::new(
        Source::plugin("signalk-rs"),
        vec![PathValue::new(path, serialized)],
    )]);

    state.store.write().await.apply_delta(delta);

    Json(serde_json::json!({"message": "ok"})).into_response()
}
