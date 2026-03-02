mod helpers;

use helpers::{get, post_empty, test_app_with_store};
use signalk_types::notification::{Notification, NotificationMethod, NotificationState};
use signalk_types::{Delta, PathValue, Source, Update};

/// Inject a notification into the store.
async fn inject_notification(
    store: &std::sync::Arc<tokio::sync::RwLock<signalk_store::store::SignalKStore>>,
    path: &str,
    notification: &Notification,
) {
    let delta = Delta::self_vessel(vec![Update::new(
        Source::plugin("test-plugin"),
        vec![PathValue::new(
            path,
            serde_json::to_value(notification).unwrap(),
        )],
    )]);
    store.write().await.apply_delta(delta);
}

fn alarm_notification() -> Notification {
    Notification {
        id: None,
        state: NotificationState::Alarm,
        method: vec![NotificationMethod::Visual, NotificationMethod::Sound],
        message: "Test alarm!".to_string(),
        status: None,
    }
}

fn emergency_notification() -> Notification {
    Notification {
        id: None,
        state: NotificationState::Emergency,
        method: vec![NotificationMethod::Visual, NotificationMethod::Sound],
        message: "Man overboard!".to_string(),
        status: None,
    }
}

// ─── GET /signalk/v2/api/notifications ──────────────────────────────────────

#[tokio::test]
async fn list_notifications_empty() {
    let (app, _store) = test_app_with_store();
    let (status, body) = get(app, "/signalk/v2/api/notifications").await;
    assert_eq!(status, 200);
    assert_eq!(body, serde_json::json!({}));
}

#[tokio::test]
async fn list_notifications_returns_active() {
    let (app, store) = test_app_with_store();

    inject_notification(
        &store,
        "notifications.navigation.anchor",
        &alarm_notification(),
    )
    .await;

    let (status, body) = get(app, "/signalk/v2/api/notifications").await;
    assert_eq!(status, 200);
    assert!(
        body.get("navigation.anchor").is_some(),
        "Expected notification in list"
    );
    assert_eq!(body["navigation.anchor"]["state"], "alarm");
}

#[tokio::test]
async fn list_notifications_multiple() {
    let (app, store) = test_app_with_store();

    inject_notification(
        &store,
        "notifications.navigation.anchor",
        &alarm_notification(),
    )
    .await;
    inject_notification(
        &store,
        "notifications.electrical.battery",
        &Notification {
            id: None,
            state: NotificationState::Warn,
            method: vec![NotificationMethod::Visual],
            message: "Low battery".to_string(),
            status: None,
        },
    )
    .await;

    let (status, body) = get(app, "/signalk/v2/api/notifications").await;
    assert_eq!(status, 200);
    let obj = body.as_object().unwrap();
    assert_eq!(obj.len(), 2);
    assert!(obj.contains_key("navigation.anchor"));
    assert!(obj.contains_key("electrical.battery"));
}

// ─── Silence API ────────────────────────────────────────────────────────────

#[tokio::test]
async fn silence_nonexistent_returns_404() {
    let (app, _store) = test_app_with_store();
    let (status, _) = post_empty(
        app,
        "/signalk/v2/api/notifications/nonexistent.alarm/silence",
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn silence_alarm_removes_sound() {
    let (app, store) = test_app_with_store();

    inject_notification(
        &store,
        "notifications.navigation.anchor",
        &alarm_notification(),
    )
    .await;

    let (status, _) = post_empty(
        app.clone(),
        "/signalk/v2/api/notifications/navigation.anchor/silence",
    )
    .await;
    assert_eq!(status, 200);

    // Verify via store
    let s = store.read().await;
    let sv = s.get_self_path("notifications.navigation.anchor").unwrap();
    let n: Notification = serde_json::from_value(sv.value.clone()).unwrap();
    assert!(!n.method.contains(&NotificationMethod::Sound));
    assert_eq!(n.status.as_ref().unwrap().silenced, Some(true));
}

#[tokio::test]
async fn silence_emergency_returns_422() {
    let (app, store) = test_app_with_store();

    inject_notification(&store, "notifications.mob", &emergency_notification()).await;

    let (status, _) = post_empty(app, "/signalk/v2/api/notifications/mob/silence").await;
    assert_eq!(status, 422);
}

// ─── Acknowledge API ────────────────────────────────────────────────────────

#[tokio::test]
async fn acknowledge_alarm_removes_sound_and_visual() {
    let (app, store) = test_app_with_store();

    inject_notification(
        &store,
        "notifications.navigation.anchor",
        &alarm_notification(),
    )
    .await;

    let (status, _) = post_empty(
        app.clone(),
        "/signalk/v2/api/notifications/navigation.anchor/acknowledge",
    )
    .await;
    assert_eq!(status, 200);

    let s = store.read().await;
    let sv = s.get_self_path("notifications.navigation.anchor").unwrap();
    let n: Notification = serde_json::from_value(sv.value.clone()).unwrap();
    assert!(n.method.is_empty());
    assert_eq!(n.status.as_ref().unwrap().acknowledged, Some(true));
}

#[tokio::test]
async fn acknowledge_emergency_keeps_visual() {
    let (app, store) = test_app_with_store();

    inject_notification(&store, "notifications.mob", &emergency_notification()).await;

    let (status, _) =
        post_empty(app.clone(), "/signalk/v2/api/notifications/mob/acknowledge").await;
    assert_eq!(status, 200);

    let s = store.read().await;
    let sv = s.get_self_path("notifications.mob").unwrap();
    let n: Notification = serde_json::from_value(sv.value.clone()).unwrap();
    // Emergency acknowledge: Sound removed, Visual kept
    assert!(!n.method.contains(&NotificationMethod::Sound));
    assert!(n.method.contains(&NotificationMethod::Visual));
    assert_eq!(n.status.as_ref().unwrap().acknowledged, Some(true));
}

// ─── Path validation ────────────────────────────────────────────────────────

#[tokio::test]
async fn invalid_notification_id_returns_400() {
    let (app, _store) = test_app_with_store();

    let (status, _) = post_empty(
        app.clone(),
        "/signalk/v2/api/notifications/..%2Fetc/silence",
    )
    .await;
    assert_eq!(status, 400);

    let (status, _) = post_empty(app, "/signalk/v2/api/notifications/.hidden/acknowledge").await;
    assert_eq!(status, 400);
}
