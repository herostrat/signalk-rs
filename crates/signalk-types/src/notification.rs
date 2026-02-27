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
/// };
/// let json = serde_json::to_value(&n).unwrap();
/// assert_eq!(json["state"], "alarm");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub state: NotificationState,
    pub method: Vec<NotificationMethod>,
    pub message: String,
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
        };
        let json = serde_json::to_value(&clear).unwrap();
        assert_eq!(json["state"], "normal");
        assert!(json["method"].as_array().unwrap().is_empty());
    }
}
