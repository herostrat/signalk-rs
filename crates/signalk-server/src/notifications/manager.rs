use signalk_types::Delta;
/// Centralized notification enrichment.
///
/// The `NotificationManager` sits in the `DeltaFilterChain` and enriches every
/// notification delta before it reaches the store:
///
/// 1. Assigns a stable UUID `id` per notification path (new UUID on first raise,
///    preserved across re-raises, reset on clear + re-raise).
/// 2. Injects `status` capability flags (`canSilence`, `canAcknowledge`, `canClear`)
///    based on the notification state and origin.
/// 3. Tracks silence/acknowledge state so API mutations stay consistent.
///
/// Uses `std::sync::RwLock` (not tokio) because `DeltaFilterChain` handlers are
/// synchronous `Fn(Delta) -> Option<Delta>`.
use signalk_types::notification::{Notification, NotificationState, NotificationStatus};
use std::collections::HashMap;
use std::sync::RwLock;
use tracing::warn;

/// Internal tracking entry for a live notification.
struct NotificationEntry {
    /// Stable UUID for this notification instance.
    id: String,
    /// Whether the notification was raised by a plugin (vs. API-originated).
    plugin_originated: bool,
    /// Whether the audible alarm has been silenced via the API.
    silenced: bool,
    /// Whether the notification has been acknowledged via the API.
    acknowledged: bool,
}

pub struct NotificationManager {
    entries: RwLock<HashMap<String, NotificationEntry>>,
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationManager {
    pub fn new() -> Self {
        NotificationManager {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Enrich a delta: inject `id` and `status` into every notification path value.
    ///
    /// Non-notification paths are passed through unchanged.
    pub fn enrich_delta(&self, mut delta: Delta) -> Delta {
        for update in &mut delta.updates {
            // A notification is "plugin-originated" if the source type is "Plugin"
            let is_plugin_source = update.source.type_ == "Plugin";

            for pv in &mut update.values {
                if !pv.path.starts_with("notifications.") {
                    continue;
                }

                // Try to deserialize as Notification
                let mut notification: Notification = match serde_json::from_value(pv.value.clone())
                {
                    Ok(n) => n,
                    Err(e) => {
                        warn!(
                            path = %pv.path,
                            error = %e,
                            "Failed to parse notification delta value, passing through"
                        );
                        continue;
                    }
                };

                let mut entries = self.entries.write().unwrap();

                if notification.state == NotificationState::Normal {
                    // Clear: remove entry, but preserve the old UUID in the clear message
                    if let Some(old) = entries.remove(&pv.path) {
                        notification.id = Some(old.id);
                    }
                    // Status on a clear notification: all capabilities false
                    notification.status = Some(NotificationStatus {
                        silenced: Some(false),
                        acknowledged: Some(false),
                        can_silence: Some(false),
                        can_acknowledge: Some(false),
                        can_clear: Some(false),
                    });
                } else {
                    // Active notification
                    let entry =
                        entries
                            .entry(pv.path.clone())
                            .or_insert_with(|| NotificationEntry {
                                id: uuid::Uuid::new_v4().to_string(),
                                plugin_originated: is_plugin_source,
                                silenced: false,
                                acknowledged: false,
                            });

                    notification.id = Some(entry.id.clone());
                    notification.status = Some(NotificationStatus {
                        silenced: Some(entry.silenced),
                        acknowledged: Some(entry.acknowledged),
                        can_silence: Some(
                            notification.state != NotificationState::Emergency && !entry.silenced,
                        ),
                        can_acknowledge: Some(!entry.acknowledged),
                        can_clear: Some(!entry.plugin_originated),
                    });
                }

                // Re-serialize
                match serde_json::to_value(&notification) {
                    Ok(v) => pv.value = v,
                    Err(e) => {
                        warn!(
                            path = %pv.path,
                            error = %e,
                            "Failed to serialize enriched notification"
                        );
                    }
                }
            }
        }
        delta
    }

    /// Mark a notification as silenced. Called by the silence API handler.
    pub fn silence(&self, path: &str) {
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.get_mut(path) {
            entry.silenced = true;
        }
    }

    /// Mark a notification as acknowledged. Called by the acknowledge API handler.
    pub fn acknowledge(&self, path: &str) {
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.get_mut(path) {
            entry.acknowledged = true;
        }
    }

    /// Get the tracked data for a notification path.
    ///
    /// Returns `(id, silenced, acknowledged, plugin_originated)` if the path is tracked.
    pub fn get_entry_data(&self, path: &str) -> Option<(String, bool, bool, bool)> {
        let entries = self.entries.read().unwrap();
        entries.get(path).map(|e| {
            (
                e.id.clone(),
                e.silenced,
                e.acknowledged,
                e.plugin_originated,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_types::notification::{NotificationMethod, NotificationState};
    use signalk_types::{PathValue, Source, Update};

    fn make_notification_delta(path: &str, notification: &Notification) -> Delta {
        Delta::self_vessel(vec![Update::new(
            Source::plugin("test-plugin"),
            vec![PathValue::new(
                path,
                serde_json::to_value(notification).unwrap(),
            )],
        )])
    }

    fn make_notification(state: NotificationState) -> Notification {
        Notification {
            id: None,
            state,
            method: vec![NotificationMethod::Visual, NotificationMethod::Sound],
            message: "Test alarm".to_string(),
            status: None,
        }
    }

    #[test]
    fn enrichment_injects_uuid_and_status() {
        let mgr = NotificationManager::new();
        let n = make_notification(NotificationState::Alarm);
        let delta = make_notification_delta("notifications.navigation.anchor", &n);

        let result = mgr.enrich_delta(delta);
        let value = &result.updates[0].values[0].value;
        let enriched: Notification = serde_json::from_value(value.clone()).unwrap();

        assert!(enriched.id.is_some());
        assert!(enriched.id.as_ref().unwrap().len() > 10); // UUID format
        let status = enriched.status.unwrap();
        assert_eq!(status.silenced, Some(false));
        assert_eq!(status.acknowledged, Some(false));
        assert_eq!(status.can_silence, Some(true));
        assert_eq!(status.can_acknowledge, Some(true));
        assert_eq!(status.can_clear, Some(false)); // plugin-originated
    }

    #[test]
    fn non_notification_paths_passed_through() {
        let mgr = NotificationManager::new();
        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![PathValue::new(
                "navigation.position",
                serde_json::json!({"latitude": 49.0, "longitude": -123.0}),
            )],
        )]);

        let result = mgr.enrich_delta(delta.clone());
        assert_eq!(
            result.updates[0].values[0].value,
            serde_json::json!({"latitude": 49.0, "longitude": -123.0})
        );
    }

    #[test]
    fn same_path_gets_same_uuid() {
        let mgr = NotificationManager::new();
        let n = make_notification(NotificationState::Alarm);

        let delta1 = make_notification_delta("notifications.navigation.anchor", &n);
        let result1 = mgr.enrich_delta(delta1);
        let id1: Notification =
            serde_json::from_value(result1.updates[0].values[0].value.clone()).unwrap();

        let delta2 = make_notification_delta("notifications.navigation.anchor", &n);
        let result2 = mgr.enrich_delta(delta2);
        let id2: Notification =
            serde_json::from_value(result2.updates[0].values[0].value.clone()).unwrap();

        assert_eq!(id1.id, id2.id);
    }

    #[test]
    fn different_paths_get_different_uuids() {
        let mgr = NotificationManager::new();
        let n = make_notification(NotificationState::Alarm);

        let delta1 = make_notification_delta("notifications.navigation.anchor", &n);
        let result1 = mgr.enrich_delta(delta1);
        let n1: Notification =
            serde_json::from_value(result1.updates[0].values[0].value.clone()).unwrap();

        let delta2 = make_notification_delta("notifications.electrical.battery", &n);
        let result2 = mgr.enrich_delta(delta2);
        let n2: Notification =
            serde_json::from_value(result2.updates[0].values[0].value.clone()).unwrap();

        assert_ne!(n1.id, n2.id);
    }

    #[test]
    fn clear_then_reraise_gets_new_uuid() {
        let mgr = NotificationManager::new();
        let path = "notifications.navigation.anchor";

        // Raise
        let n = make_notification(NotificationState::Alarm);
        let result = mgr.enrich_delta(make_notification_delta(path, &n));
        let first: Notification =
            serde_json::from_value(result.updates[0].values[0].value.clone()).unwrap();

        // Clear
        let clear = Notification {
            id: None,
            state: NotificationState::Normal,
            method: vec![],
            message: String::new(),
            status: None,
        };
        let clear_result = mgr.enrich_delta(make_notification_delta(path, &clear));
        let cleared: Notification =
            serde_json::from_value(clear_result.updates[0].values[0].value.clone()).unwrap();
        // Clear message carries old UUID
        assert_eq!(cleared.id, first.id);

        // Re-raise
        let result = mgr.enrich_delta(make_notification_delta(path, &n));
        let second: Notification =
            serde_json::from_value(result.updates[0].values[0].value.clone()).unwrap();

        assert_ne!(first.id, second.id);
    }

    #[test]
    fn emergency_cannot_be_silenced() {
        let mgr = NotificationManager::new();
        let n = make_notification(NotificationState::Emergency);
        let delta = make_notification_delta("notifications.mob", &n);

        let result = mgr.enrich_delta(delta);
        let enriched: Notification =
            serde_json::from_value(result.updates[0].values[0].value.clone()).unwrap();

        assert_eq!(enriched.status.as_ref().unwrap().can_silence, Some(false));
        assert_eq!(
            enriched.status.as_ref().unwrap().can_acknowledge,
            Some(true)
        );
    }

    #[test]
    fn silence_updates_internal_state() {
        let mgr = NotificationManager::new();
        let path = "notifications.navigation.anchor";

        // Raise
        let n = make_notification(NotificationState::Alarm);
        mgr.enrich_delta(make_notification_delta(path, &n));

        // Silence via manager
        mgr.silence(path);

        let (_, silenced, acknowledged, _) = mgr.get_entry_data(path).unwrap();
        assert!(silenced);
        assert!(!acknowledged);

        // Next enrichment reflects silenced state
        let result = mgr.enrich_delta(make_notification_delta(path, &n));
        let enriched: Notification =
            serde_json::from_value(result.updates[0].values[0].value.clone()).unwrap();
        let status = enriched.status.unwrap();
        assert_eq!(status.silenced, Some(true));
        assert_eq!(status.can_silence, Some(false)); // already silenced
    }

    #[test]
    fn acknowledge_updates_internal_state() {
        let mgr = NotificationManager::new();
        let path = "notifications.navigation.anchor";

        let n = make_notification(NotificationState::Alarm);
        mgr.enrich_delta(make_notification_delta(path, &n));

        mgr.acknowledge(path);

        let (_, _, acknowledged, _) = mgr.get_entry_data(path).unwrap();
        assert!(acknowledged);

        let result = mgr.enrich_delta(make_notification_delta(path, &n));
        let enriched: Notification =
            serde_json::from_value(result.updates[0].values[0].value.clone()).unwrap();
        let status = enriched.status.unwrap();
        assert_eq!(status.acknowledged, Some(true));
        assert_eq!(status.can_acknowledge, Some(false)); // already acknowledged
    }

    #[test]
    fn get_entry_data_returns_none_for_unknown() {
        let mgr = NotificationManager::new();
        assert!(mgr.get_entry_data("notifications.unknown").is_none());
    }

    #[test]
    fn clear_notification_status_all_false() {
        let mgr = NotificationManager::new();
        let path = "notifications.navigation.anchor";

        // Raise first
        let n = make_notification(NotificationState::Alarm);
        mgr.enrich_delta(make_notification_delta(path, &n));

        // Clear
        let clear = Notification {
            id: None,
            state: NotificationState::Normal,
            method: vec![],
            message: String::new(),
            status: None,
        };
        let result = mgr.enrich_delta(make_notification_delta(path, &clear));
        let enriched: Notification =
            serde_json::from_value(result.updates[0].values[0].value.clone()).unwrap();

        let status = enriched.status.unwrap();
        assert_eq!(status.can_silence, Some(false));
        assert_eq!(status.can_acknowledge, Some(false));
        assert_eq!(status.can_clear, Some(false));
    }

    #[test]
    fn mixed_notification_and_regular_paths_in_same_update() {
        let mgr = NotificationManager::new();
        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![
                PathValue::new("navigation.position", serde_json::json!({"latitude": 49.0})),
                PathValue::new(
                    "notifications.navigation.anchor",
                    serde_json::to_value(make_notification(NotificationState::Alarm)).unwrap(),
                ),
            ],
        )]);

        let result = mgr.enrich_delta(delta);
        // Position unchanged
        assert_eq!(
            result.updates[0].values[0].value,
            serde_json::json!({"latitude": 49.0})
        );
        // Notification enriched
        let enriched: Notification =
            serde_json::from_value(result.updates[0].values[1].value.clone()).unwrap();
        assert!(enriched.id.is_some());
    }
}
