//! DSC (Digital Selective Calling) sentence decoder.
//!
//! DSC sentences carry VHF radio calls with MMSI, position, and distress info.
//! Unlike standard NMEA sentences, DSC produces deltas for **other vessels**
//! (with MMSI-based context). Distress calls additionally set a spec-conformant
//! notification (e.g. `notifications.fire`) on the calling vessel.

use serde_json::json;
use signalk_types::notification::{Notification, NotificationMethod, NotificationState};
use signalk_types::{Delta, PathValue, Source, Update};
use tracing::debug;

/// Map DSC "nature of distress" code to SignalK spec notification path.
///
/// These are the reserved top-level notification keys from the SignalK spec.
/// See: <https://signalk.org/specification/1.7.0/doc/notifications.html>
fn distress_notification_path(nature_of_distress: Option<u8>) -> &'static str {
    match nature_of_distress {
        Some(0) => "notifications.fire",
        Some(1) => "notifications.flooding",
        Some(2) => "notifications.collision",
        Some(3) => "notifications.grounding",
        Some(4) => "notifications.listing",
        Some(5) => "notifications.sinking",
        Some(6) => "notifications.adrift",
        Some(8) => "notifications.abandon",
        Some(9) => "notifications.piracy",
        Some(10) => "notifications.mob",
        _ => "notifications.distress",
    }
}

/// Human-readable label for the nature of distress.
fn distress_nature_label(nature_of_distress: Option<u8>) -> &'static str {
    match nature_of_distress {
        Some(0) => "Fire/Explosion",
        Some(1) => "Flooding",
        Some(2) => "Collision",
        Some(3) => "Grounding",
        Some(4) => "Listing/Capsizing",
        Some(5) => "Sinking",
        Some(6) => "Adrift",
        Some(8) => "Abandon ship",
        Some(9) => "Piracy",
        Some(10) => "Man overboard",
        Some(12) => "EPIRB emission",
        _ => "Undesignated distress",
    }
}

/// Try to decode a DSC sentence into SignalK deltas.
///
/// Returns `None` if the sentence is not a DSC sentence or fails to parse.
/// Returns `Some(vec)` with 1-2 deltas:
/// - Always: a vessel delta (context = other vessel) with position if available
/// - If distress: a notification delta on the other vessel with the spec-conformant
///   path (e.g. `notifications.fire`, `notifications.mob`)
pub fn try_decode_dsc(raw: &str, source_label: &str) -> Option<Vec<Delta>> {
    // Quick pre-check: DSC sentences contain "DSC" in the talker+type
    if !raw.contains("DSC") {
        return None;
    }

    let parsed = nmea::parse_str(raw)
        .map_err(|e| debug!(sentence = %raw, "DSC parse error: {e:?}"))
        .ok()?;

    let dsc = match parsed {
        nmea::ParseResult::DSC(dsc) => dsc,
        _ => return None,
    };

    let mmsi = dsc.sender_mmsi()?;
    let context = format!("vessels.urn:mrn:imo:mmsi:{mmsi:09}");
    let source = Source::nmea0183(source_label, "DS");

    // Collect values for the other vessel's delta
    let mut values = Vec::new();

    // Position (if decodable)
    if let Some((lat, lon)) = dsc.decode_position() {
        values.push(PathValue::new(
            "navigation.position",
            json!({"latitude": lat, "longitude": lon}),
        ));
    }

    // Distress notification on the calling vessel
    if dsc.category == Some(12) {
        let nature = dsc.first_telecommand;
        let path = distress_notification_path(nature);
        let nature_label = distress_nature_label(nature);

        let position_str = dsc
            .decode_position()
            .map(|(lat, lon)| format!(" at {lat:.4},{lon:.4}"))
            .unwrap_or_default();
        let message = format!("DSC Distress: {nature_label} from MMSI {mmsi:09}{position_str}");

        let notification = Notification {
            state: NotificationState::Emergency,
            method: vec![NotificationMethod::Visual, NotificationMethod::Sound],
            message,
        };

        values.push(PathValue::new(
            path,
            serde_json::to_value(&notification).unwrap(),
        ));
    }

    if values.is_empty() {
        return None;
    }

    let update = Update::new(source, values);
    Some(vec![Delta::with_context(&context, vec![update])])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_dsc_returns_none() {
        assert!(
            try_decode_dsc(
                "$GPRMC,225446.33,A,4916.45,N,12311.12,W,000.5,054.7,191194,020.3,E,A*2B",
                "test"
            )
            .is_none()
        );
    }

    #[test]
    fn empty_returns_none() {
        assert!(try_decode_dsc("", "test").is_none());
    }

    #[test]
    fn garbage_dsc_returns_none() {
        assert!(try_decode_dsc("$GPDSC,garbage", "test").is_none());
    }

    #[test]
    fn distress_paths() {
        assert_eq!(distress_notification_path(Some(0)), "notifications.fire");
        assert_eq!(
            distress_notification_path(Some(1)),
            "notifications.flooding"
        );
        assert_eq!(
            distress_notification_path(Some(2)),
            "notifications.collision"
        );
        assert_eq!(
            distress_notification_path(Some(3)),
            "notifications.grounding"
        );
        assert_eq!(distress_notification_path(Some(4)), "notifications.listing");
        assert_eq!(distress_notification_path(Some(5)), "notifications.sinking");
        assert_eq!(distress_notification_path(Some(6)), "notifications.adrift");
        assert_eq!(distress_notification_path(Some(8)), "notifications.abandon");
        assert_eq!(distress_notification_path(Some(9)), "notifications.piracy");
        assert_eq!(distress_notification_path(Some(10)), "notifications.mob");
        assert_eq!(distress_notification_path(None), "notifications.distress");
        assert_eq!(
            distress_notification_path(Some(99)),
            "notifications.distress"
        );
    }

    #[test]
    fn distress_labels() {
        assert_eq!(distress_nature_label(Some(0)), "Fire/Explosion");
        assert_eq!(distress_nature_label(Some(5)), "Sinking");
        assert_eq!(distress_nature_label(Some(10)), "Man overboard");
        assert_eq!(distress_nature_label(Some(12)), "EPIRB emission");
        assert_eq!(distress_nature_label(None), "Undesignated distress");
    }

    #[test]
    fn notification_has_required_fields() {
        let notification = Notification {
            state: NotificationState::Emergency,
            method: vec![NotificationMethod::Visual, NotificationMethod::Sound],
            message: "DSC Distress: Fire/Explosion from MMSI 211457160".to_string(),
        };
        let json = serde_json::to_value(&notification).unwrap();
        assert_eq!(json["state"], "emergency");
        assert_eq!(json["method"][0], "visual");
        assert_eq!(json["method"][1], "sound");
        assert!(json["message"].as_str().unwrap().contains("Fire"));
    }

    #[test]
    fn mmsi_context_format() {
        let context = format!("vessels.urn:mrn:imo:mmsi:{:09}", 211457160u32);
        assert_eq!(context, "vessels.urn:mrn:imo:mmsi:211457160");
    }

    #[test]
    fn mmsi_context_format_short() {
        let context = format!("vessels.urn:mrn:imo:mmsi:{:09}", 1234u32);
        assert_eq!(context, "vessels.urn:mrn:imo:mmsi:000001234");
    }
}
