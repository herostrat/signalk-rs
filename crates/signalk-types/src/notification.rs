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
    pub state: NotificationState,
    pub method: Vec<NotificationMethod>,
    pub message: String,
    /// Client interaction state (silenced, acknowledged). Set via the v2 Notifications API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<NotificationStatus>,
}

/// Client interaction state for a notification.
///
/// Set by the v2 Notifications API endpoints:
/// - `POST /signalk/v2/api/notifications/{id}/silence`
/// - `POST /signalk/v2/api/notifications/{id}/acknowledge`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NotificationStatus {
    /// Whether the audible alarm has been silenced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silenced: Option<bool>,
    /// Whether the alarm has been acknowledged (no further alerts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<bool>,
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
            state: NotificationState::Alarm,
            method: vec![NotificationMethod::Sound],
            message: "test".to_string(),
            status: Some(NotificationStatus {
                silenced: Some(true),
                acknowledged: None,
            }),
        };
        let json = serde_json::to_value(&n).unwrap();
        assert_eq!(json["status"]["silenced"], true);
        assert!(json["status"].get("acknowledged").is_none());
    }
}
