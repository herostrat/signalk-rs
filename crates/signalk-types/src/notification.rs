/// SignalK notification value objects.
///
/// Notifications live under `notifications.*` paths in the data model and
/// represent alarm/warning conditions. Plugins raise them via
/// `PluginContext::raise_notification()`.
///
/// Spec: <https://signalk.org/specification/1.7.0/doc/notifications.html>
use serde::{Deserialize, Serialize};

/// A SignalK notification value.
///
/// # Example
///
/// ```
/// use signalk_types::notification::*;
///
/// let n = Notification {
///     id: None,
///     state: NotificationState::Alarm,
///     method: vec![NotificationMethod::Visual, NotificationMethod::Sound],
///     message: "Anchor alarm!".to_string(),
///     status: None,
/// };
/// let json = serde_json::to_value(&n).unwrap();
/// assert_eq!(json["state"], "alarm");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// Unique identifier assigned by the NotificationManager. Not set by plugins directly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub state: NotificationState,
    pub method: Vec<NotificationMethod>,
    pub message: String,
    /// Client interaction state and capability flags. Enriched by the NotificationManager.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<NotificationStatus>,
}

/// Client interaction state and capability flags for a notification.
///
/// Set by the NotificationManager (capability flags) and mutated by the
/// v2 Notifications API endpoints:
/// - `POST /signalk/v2/api/notifications/{id}/silence`
/// - `POST /signalk/v2/api/notifications/{id}/acknowledge`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NotificationStatus {
    /// Whether the audible alarm has been silenced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silenced: Option<bool>,
    /// Whether the alarm has been acknowledged (no further alerts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<bool>,
    /// Whether this notification can be silenced (false for Emergency).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_silence: Option<bool>,
    /// Whether this notification can be acknowledged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_acknowledge: Option<bool>,
    /// Whether this notification can be cleared via the API (false for plugin-originated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_clear: Option<bool>,
}

impl NotificationStatus {
    /// Create an initial status for a newly raised notification.
    ///
    /// `from_api` controls `can_clear`: notifications raised via the REST API
    /// can be cleared, while plugin-originated ones cannot (only the plugin
    /// lifecycle can clear them).
    pub fn initial(state: NotificationState, from_api: bool) -> Self {
        NotificationStatus {
            silenced: Some(false),
            acknowledged: Some(false),
            can_silence: Some(state != NotificationState::Emergency),
            can_acknowledge: Some(true),
            can_clear: Some(from_api),
        }
    }
}

/// Notification severity state.
///
/// Ordered by increasing severity: Normal → Nominal → Alert → Warn → Alarm → Emergency.
/// `Normal` clears an active notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationState {
    /// Default — no alarm condition. Used to clear a previous notification.
    Normal,
    /// Within an acceptable "nominal zone".
    Nominal,
    /// Safe condition brought to operator's attention for routine action.
    Alert,
    /// Condition requiring attention but not immediate action.
    Warn,
    /// Condition outside acceptable range — immediate action required.
    Alarm,
    /// Life-threatening condition.
    Emergency,
}

/// Notification delivery method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationMethod {
    /// Display alert on screen/UI.
    Visual,
    /// Trigger audible alarm.
    Sound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_serde_roundtrip() {
        let n = Notification {
            id: None,
            state: NotificationState::Alarm,
            method: vec![NotificationMethod::Visual, NotificationMethod::Sound],
            message: "Anchor dragging!".to_string(),
            status: None,
        };
        let json = serde_json::to_string(&n).unwrap();
        let parsed: Notification = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, n);
    }

    #[test]
    fn notification_state_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(NotificationState::Emergency)
                .unwrap()
                .as_str()
                .unwrap(),
            "emergency"
        );
        assert_eq!(
            serde_json::to_value(NotificationState::Normal)
                .unwrap()
                .as_str()
                .unwrap(),
            "normal"
        );
    }

    #[test]
    fn notification_method_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(NotificationMethod::Visual)
                .unwrap()
                .as_str()
                .unwrap(),
            "visual"
        );
        assert_eq!(
            serde_json::to_value(NotificationMethod::Sound)
                .unwrap()
                .as_str()
                .unwrap(),
            "sound"
        );
    }

    #[test]
    fn normal_notification_clears_alarm() {
        let clear = Notification {
            id: None,
            state: NotificationState::Normal,
            method: vec![],
            message: String::new(),
            status: None,
        };
        let json = serde_json::to_value(&clear).unwrap();
        assert_eq!(json["state"], "normal");
        assert!(json["method"].as_array().unwrap().is_empty());
    }

    #[test]
    fn status_not_serialized_when_none() {
        let n = Notification {
            id: None,
            state: NotificationState::Alarm,
            method: vec![NotificationMethod::Sound],
            message: "test".to_string(),
            status: None,
        };
        let json = serde_json::to_value(&n).unwrap();
        assert!(json.get("status").is_none());
    }

    #[test]
    fn status_serialized_when_set() {
        let n = Notification {
            id: None,
            state: NotificationState::Alarm,
            method: vec![NotificationMethod::Sound],
            message: "test".to_string(),
            status: Some(NotificationStatus {
                silenced: Some(true),
                acknowledged: None,
                ..Default::default()
            }),
        };
        let json = serde_json::to_value(&n).unwrap();
        assert_eq!(json["status"]["silenced"], true);
        assert!(json["status"].get("acknowledged").is_none());
    }

    #[test]
    fn id_not_serialized_when_none() {
        let n = Notification {
            id: None,
            state: NotificationState::Alarm,
            method: vec![NotificationMethod::Sound],
            message: "test".to_string(),
            status: None,
        };
        let json = serde_json::to_value(&n).unwrap();
        assert!(json.get("id").is_none());
    }

    #[test]
    fn id_serialized_when_present() {
        let n = Notification {
            id: Some("abc-123".to_string()),
            state: NotificationState::Alarm,
            method: vec![NotificationMethod::Sound],
            message: "test".to_string(),
            status: None,
        };
        let json = serde_json::to_value(&n).unwrap();
        assert_eq!(json["id"], "abc-123");
    }

    #[test]
    fn capability_flags_serialize_camel_case() {
        let status = NotificationStatus {
            silenced: Some(false),
            acknowledged: Some(false),
            can_silence: Some(true),
            can_acknowledge: Some(true),
            can_clear: Some(false),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["canSilence"], true);
        assert_eq!(json["canAcknowledge"], true);
        assert_eq!(json["canClear"], false);
        // Verify camelCase (not snake_case)
        assert!(json.get("can_silence").is_none());
    }

    #[test]
    fn initial_status_alarm() {
        let status = NotificationStatus::initial(NotificationState::Alarm, false);
        assert_eq!(status.can_silence, Some(true));
        assert_eq!(status.can_acknowledge, Some(true));
        assert_eq!(status.can_clear, Some(false));
        assert_eq!(status.silenced, Some(false));
        assert_eq!(status.acknowledged, Some(false));
    }

    #[test]
    fn initial_status_emergency_cannot_silence() {
        let status = NotificationStatus::initial(NotificationState::Emergency, false);
        assert_eq!(status.can_silence, Some(false));
        assert_eq!(status.can_acknowledge, Some(true));
    }

    #[test]
    fn initial_status_api_originated_can_clear() {
        let status = NotificationStatus::initial(NotificationState::Warn, true);
        assert_eq!(status.can_clear, Some(true));
    }
}
