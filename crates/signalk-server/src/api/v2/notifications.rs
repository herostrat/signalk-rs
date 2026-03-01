/// Notifications API handlers.
///
/// | Method | Route | Handler |
/// |--------|-------|---------|
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
    mutate_notification(&state, &store_path, |n| {
        if n.state == NotificationState::Emergency {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "Emergency notifications cannot be silenced".to_string(),
            ));
        }
        n.method.retain(|m| *m != NotificationMethod::Sound);
        n.status
            .get_or_insert_with(NotificationStatus::default)
            .silenced = Some(true);
        Ok(())
    })
    .await
}

/// `POST /signalk/v2/api/notifications/{notification_id}/acknowledge`
///
/// Removes `Sound` and `Visual` from the method array and sets `status.acknowledged = true`.
pub async fn acknowledge(
    State(state): State<Arc<ServerState>>,
    Path(notification_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = validate_notification_id(&notification_id) {
        return e.into_response();
    }
    let store_path = format!("notifications.{notification_id}");
    mutate_notification(&state, &store_path, |n| {
        n.method
            .retain(|m| !matches!(m, NotificationMethod::Sound | NotificationMethod::Visual));
        n.status
            .get_or_insert_with(NotificationStatus::default)
            .acknowledged = Some(true);
        Ok(())
    })
    .await
}

/// Read a notification from the store, apply `mutate`, write it back as a delta.
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

    // Write back as delta
    let delta = Delta::self_vessel(vec![Update::new(
        Source::plugin("signalk-rs"),
        vec![PathValue::new(path, serialized)],
    )]);

    state.store.write().await.apply_delta(delta);

    Json(serde_json::json!({"message": "ok"})).into_response()
}
