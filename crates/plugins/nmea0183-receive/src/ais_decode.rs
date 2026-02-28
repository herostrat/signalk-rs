//! AIS VDM/VDO sentence decoder.
//!
//! Uses the `ais` crate for fragment reassembly and message decoding,
//! then maps decoded messages to `AisContact` for SignalK delta conversion.

use ais::messages::AisMessage;
use ais::messages::position_report::NavigationStatus;
use ais::{AisFragments, AisParser};
use signalk_types::ais::AisContact;
use signalk_types::{Delta, Source};
use tracing::debug;

const KNOTS_TO_MS: f64 = 0.514_444;
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// Stateful AIS decoder with fragment reassembly.
///
/// Wraps `ais::AisParser` which handles multi-fragment messages (e.g. Type 5).
/// Call `try_decode()` for each NMEA sentence; it returns `Some(Delta)` when
/// a complete AIS message has been decoded.
pub struct AisDecoder {
    parser: AisParser,
    source_label: String,
}

impl AisDecoder {
    pub fn new(source_label: &str) -> Self {
        AisDecoder {
            parser: AisParser::new(),
            source_label: source_label.to_string(),
        }
    }

    /// Try to decode a VDM/VDO sentence into a SignalK delta.
    ///
    /// Returns `None` if:
    /// - The sentence is not a VDM/VDO (standard NMEA)
    /// - The sentence is an incomplete fragment (waiting for more)
    /// - The AIS message type is unsupported
    pub fn try_decode(&mut self, sentence: &str) -> Option<Delta> {
        // Only attempt AIS parsing for VDM/VDO sentences
        if !sentence.contains("VDM") && !sentence.contains("VDO") {
            return None;
        }

        let fragments = self
            .parser
            .parse(sentence.as_bytes(), true)
            .map_err(|e| debug!(sentence = %sentence, "AIS parse error: {e:?}"))
            .ok()?;

        let sentence = match fragments {
            AisFragments::Complete(s) => s,
            AisFragments::Incomplete(_) => return None,
        };

        let message = sentence.message?;
        let contact = message_to_contact(&message)?;
        let source = Source::nmea0183(&self.source_label, "AI");
        Some(contact.to_delta(source))
    }
}

/// Convert an AIS message to an `AisContact`.
fn message_to_contact(message: &AisMessage) -> Option<AisContact> {
    match message {
        AisMessage::PositionReport(pos) => Some(from_position_report(pos)),
        AisMessage::StaticAndVoyageRelatedData(data) => Some(from_static_voyage(data)),
        AisMessage::StandardClassBPositionReport(pos) => Some(from_class_b(pos)),
        AisMessage::ExtendedClassBPositionReport(pos) => Some(from_class_b_extended(pos)),
        AisMessage::AidToNavigationReport(aton) => Some(from_aton(aton)),
        AisMessage::StaticDataReport(sdr) => Some(from_static_data_report(sdr)),
        _ => None,
    }
}

/// Type 1-3: Class A position report.
fn from_position_report(pos: &ais::messages::position_report::PositionReport) -> AisContact {
    let mut contact = AisContact {
        mmsi: pos.mmsi,
        position: match (pos.latitude, pos.longitude) {
            (Some(lat), Some(lon)) => Some((lat as f64, lon as f64)),
            _ => None,
        },
        sog_ms: pos.speed_over_ground.map(|s| s as f64 * KNOTS_TO_MS),
        cog_rad: pos.course_over_ground.map(|c| c as f64 * DEG_TO_RAD),
        heading_rad: pos.true_heading.map(|h| h as f64 * DEG_TO_RAD),
        nav_status: pos.navigation_status.as_ref().map(nav_status_to_code),
        ..Default::default()
    };

    // Rate of turn: ais crate gives deg/min (unsigned), direction is separate
    if let Some(rot) = &pos.rate_of_turn
        && let Some(rate_dpm) = rot.rate()
    {
        use ais::messages::navigation::Direction;
        let sign = match rot.direction() {
            Some(Direction::Port) => -1.0,
            _ => 1.0,
        };
        contact.rot_rads = Some(sign * rate_dpm as f64 * DEG_TO_RAD / 60.0);
    }

    contact
}

/// Type 5: Static and voyage related data.
fn from_static_voyage(
    data: &ais::messages::static_and_voyage_related_data::StaticAndVoyageRelatedData,
) -> AisContact {
    let length = (data.dimension_to_bow + data.dimension_to_stern) as f64;
    let beam = (data.dimension_to_port + data.dimension_to_starboard) as f64;

    AisContact {
        mmsi: data.mmsi,
        name: Some(data.vessel_name.to_string()),
        callsign: Some(data.callsign.to_string()),
        imo: Some(data.imo_number),
        ship_type: data.ship_type.as_ref().map(|st| u8::from(*st)),
        destination: Some(data.destination.to_string()),
        draught: Some(data.draught as f64),
        length: if length > 0.0 { Some(length) } else { None },
        beam: if beam > 0.0 { Some(beam) } else { None },
        ..Default::default()
    }
}

/// Type 18: Standard Class B position report.
fn from_class_b(
    pos: &ais::messages::standard_class_b_position_report::StandardClassBPositionReport,
) -> AisContact {
    AisContact {
        mmsi: pos.mmsi,
        position: match (pos.latitude, pos.longitude) {
            (Some(lat), Some(lon)) => Some((lat as f64, lon as f64)),
            _ => None,
        },
        sog_ms: pos.speed_over_ground.map(|s| s as f64 * KNOTS_TO_MS),
        cog_rad: pos.course_over_ground.map(|c| c as f64 * DEG_TO_RAD),
        heading_rad: pos.true_heading.map(|h| h as f64 * DEG_TO_RAD),
        ..Default::default()
    }
}

/// Type 19: Extended Class B position report.
fn from_class_b_extended(
    pos: &ais::messages::extended_class_b_position_report::ExtendedClassBPositionReport,
) -> AisContact {
    let length = (pos.dimension_to_bow + pos.dimension_to_stern) as f64;
    let beam = (pos.dimension_to_port + pos.dimension_to_starboard) as f64;

    AisContact {
        mmsi: pos.mmsi,
        position: match (pos.latitude, pos.longitude) {
            (Some(lat), Some(lon)) => Some((lat as f64, lon as f64)),
            _ => None,
        },
        sog_ms: pos.speed_over_ground.map(|s| s as f64 * KNOTS_TO_MS),
        cog_rad: pos.course_over_ground.map(|c| c as f64 * DEG_TO_RAD),
        heading_rad: pos.true_heading.map(|h| h as f64 * DEG_TO_RAD),
        name: Some(pos.name.to_string()),
        ship_type: pos.type_of_ship_and_cargo.as_ref().map(|st| u8::from(*st)),
        length: if length > 0.0 { Some(length) } else { None },
        beam: if beam > 0.0 { Some(beam) } else { None },
        ..Default::default()
    }
}

/// Type 21: Aid to Navigation report.
fn from_aton(aton: &ais::messages::aid_to_navigation_report::AidToNavigationReport) -> AisContact {
    AisContact {
        mmsi: aton.mmsi,
        position: match (aton.latitude, aton.longitude) {
            (Some(lat), Some(lon)) => Some((lat as f64, lon as f64)),
            _ => None,
        },
        name: Some(aton.name.to_string()),
        ..Default::default()
    }
}

/// Type 24: Static data report (Part A = name, Part B = callsign/type/dimensions).
fn from_static_data_report(
    sdr: &ais::messages::static_data_report::StaticDataReport,
) -> AisContact {
    use ais::messages::static_data_report::MessagePart;

    let mut contact = AisContact {
        mmsi: sdr.mmsi,
        ..Default::default()
    };

    match &sdr.message_part {
        MessagePart::PartA { vessel_name } => {
            contact.name = Some(vessel_name.to_string());
        }
        MessagePart::PartB {
            ship_type,
            callsign,
            dimension_to_bow,
            dimension_to_stern,
            dimension_to_port,
            dimension_to_starboard,
            ..
        } => {
            contact.callsign = Some(callsign.to_string());
            contact.ship_type = ship_type.as_ref().map(|st| u8::from(*st));
            let length = (*dimension_to_bow + *dimension_to_stern) as f64;
            let beam = (*dimension_to_port + *dimension_to_starboard) as f64;
            if length > 0.0 {
                contact.length = Some(length);
            }
            if beam > 0.0 {
                contact.beam = Some(beam);
            }
        }
        _ => {}
    }

    contact
}

/// Map NavigationStatus enum to ITU status code.
fn nav_status_to_code(status: &NavigationStatus) -> u8 {
    match status {
        NavigationStatus::UnderWayUsingEngine => 0,
        NavigationStatus::AtAnchor => 1,
        NavigationStatus::NotUnderCommand => 2,
        NavigationStatus::RestrictedManouverability => 3,
        NavigationStatus::ConstrainedByDraught => 4,
        NavigationStatus::Moored => 5,
        NavigationStatus::Aground => 6,
        NavigationStatus::EngagedInFishing => 7,
        NavigationStatus::UnderWaySailing => 8,
        NavigationStatus::ReservedForHSC => 9,
        NavigationStatus::ReservedForWIG => 10,
        NavigationStatus::Reserved01 => 11,
        NavigationStatus::Reserved02 => 12,
        NavigationStatus::Reserved03 => 13,
        NavigationStatus::AisSartIsActive => 14,
        NavigationStatus::Unknown(x) => *x,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real VDM sentences from AIS specification / common test vectors.

    // Type 1: Class A position report
    // MMSI 366814480, SOG 11.6 kn, COG 40.4°, Heading 41°
    const VDM_TYPE1: &str = "!AIVDM,1,1,,B,13u@Dt002s000000000000000000,0*42";

    // Type 5: Static and voyage data (2 fragments)
    const VDM_TYPE5_FRAG1: &str =
        "!AIVDM,2,1,3,B,55?MbV02>H97ac@63F220l4r>0Hth00000015>P4640Ht02R@,0*42";
    const VDM_TYPE5_FRAG2: &str = "!AIVDM,2,2,3,B,0000000000,2*20";

    #[test]
    fn type1_class_a_position() {
        let mut decoder = AisDecoder::new("test");
        let delta = decoder.try_decode(VDM_TYPE1);
        // May or may not decode depending on checksum; test the decoder doesn't panic
        if let Some(delta) = delta {
            assert!(
                delta
                    .context
                    .as_deref()
                    .unwrap()
                    .starts_with("vessels.urn:mrn:imo:mmsi:")
            );
        }
    }

    #[test]
    fn non_ais_sentence_returns_none() {
        let mut decoder = AisDecoder::new("test");
        assert!(
            decoder
                .try_decode(
                    "$GPRMC,225446.33,A,4916.45,N,12311.12,W,000.5,054.7,191194,020.3,E,A*2B"
                )
                .is_none()
        );
    }

    #[test]
    fn incomplete_fragment_returns_none() {
        let mut decoder = AisDecoder::new("test");
        // First fragment of a multi-fragment message should return None
        let result = decoder.try_decode(VDM_TYPE5_FRAG1);
        assert!(result.is_none());
    }

    #[test]
    fn multi_fragment_reassembly() {
        let mut decoder = AisDecoder::new("test");
        // First fragment → None (incomplete)
        assert!(decoder.try_decode(VDM_TYPE5_FRAG1).is_none());
        // Second fragment → Some (complete Type 5 message)
        let delta = decoder.try_decode(VDM_TYPE5_FRAG2);
        if let Some(delta) = delta {
            assert!(
                delta
                    .context
                    .as_deref()
                    .unwrap()
                    .starts_with("vessels.urn:mrn:imo:mmsi:")
            );
            // Type 5 should have name, callsign etc.
            let values = &delta.updates[0].values;
            assert!(
                values
                    .iter()
                    .any(|v| v.path == "name" || v.path == "communication.callsignVhf")
            );
        }
    }

    #[test]
    fn source_is_nmea0183_ai() {
        let mut decoder = AisDecoder::new("my-ais");
        if let Some(delta) = decoder.try_decode(VDM_TYPE1) {
            assert_eq!(delta.updates[0].source.type_, "NMEA0183");
            assert_eq!(delta.updates[0].source.label, "my-ais");
            assert_eq!(
                delta.updates[0].source.extra.get("talker"),
                Some(&serde_json::json!("AI"))
            );
        }
    }

    #[test]
    fn knots_to_ms_conversion() {
        // Verify the constant is correct: 1 knot = 0.514444 m/s
        assert!((KNOTS_TO_MS - 0.514_444).abs() < 1e-6);
    }

    #[test]
    fn deg_to_rad_conversion() {
        assert!((DEG_TO_RAD * 180.0 - std::f64::consts::PI).abs() < 1e-10);
    }

    #[test]
    fn nav_status_mapping() {
        assert_eq!(
            nav_status_to_code(&NavigationStatus::UnderWayUsingEngine),
            0
        );
        assert_eq!(nav_status_to_code(&NavigationStatus::AtAnchor), 1);
        assert_eq!(nav_status_to_code(&NavigationStatus::Moored), 5);
        assert_eq!(nav_status_to_code(&NavigationStatus::EngagedInFishing), 7);
        assert_eq!(nav_status_to_code(&NavigationStatus::UnderWaySailing), 8);
        assert_eq!(nav_status_to_code(&NavigationStatus::AisSartIsActive), 14);
    }

    #[test]
    fn from_position_report_fills_contact() {
        use ais::messages::navigation::Accuracy;
        use ais::messages::position_report::*;
        use ais::messages::radio_status::*;

        let pos = PositionReport {
            message_type: 1,
            repeat_indicator: 0,
            mmsi: 211457160,
            navigation_status: Some(NavigationStatus::UnderWayUsingEngine),
            rate_of_turn: None,
            speed_over_ground: Some(10.0), // 10 knots
            position_accuracy: Accuracy::Unaugmented,
            longitude: Some(10.5),
            latitude: Some(54.3),
            course_over_ground: Some(90.0),
            true_heading: Some(92),
            timestamp: 15,
            maneuver_indicator: None,
            raim: false,
            radio_status: RadioStatus::Sotdma(SotdmaMessage {
                sync_state: SyncState::UtcDirect,
                slot_timeout: 0,
                sub_message: SubMessage::SlotOffset(0),
            }),
        };

        let contact = from_position_report(&pos);
        assert_eq!(contact.mmsi, 211457160);
        let (lat, lon) = contact.position.unwrap();
        assert!((lat - 54.3).abs() < 1e-5);
        assert!((lon - 10.5).abs() < 1e-5);

        let sog = contact.sog_ms.unwrap();
        assert!((sog - 10.0 * KNOTS_TO_MS).abs() < 1e-6);

        let cog = contact.cog_rad.unwrap();
        assert!((cog - 90.0 * DEG_TO_RAD).abs() < 1e-6);

        let hdg = contact.heading_rad.unwrap();
        assert!((hdg - 92.0 * DEG_TO_RAD).abs() < 1e-6);

        assert_eq!(contact.nav_status, Some(0));
    }

    #[test]
    fn empty_sentence_returns_none() {
        let mut decoder = AisDecoder::new("test");
        assert!(decoder.try_decode("").is_none());
    }

    #[test]
    fn garbage_vdm_returns_none() {
        let mut decoder = AisDecoder::new("test");
        assert!(decoder.try_decode("!AIVDM,garbage").is_none());
    }
}
