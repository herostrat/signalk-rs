//! AIS contact → SignalK delta conversion.
//!
//! Provides `AisContact`, a source-agnostic intermediate type that both NMEA 0183
//! (via `ais` crate VDM decode) and NMEA 2000 (via PGN 129038–129810) populate.
//! `AisContact::to_delta()` produces a spec-conformant SignalK delta.
//!
//! All numeric fields use SI units (m/s, radians, meters).

use crate::delta::{Delta, PathValue, Update};
use crate::source::Source;

// ─── MMSI Classification (ITU-R M.585) ──────────────────────────────────────

/// AIS target class based on MMSI prefix (ITU-R M.585).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetClass {
    /// Normal vessel (Class A or B — not distinguishable from MMSI alone)
    Vessel,
    /// Aid to Navigation (MMSI prefix 970–979)
    Aton,
    /// Coast/Base station (MMSI prefix 00)
    Base,
    /// Search and Rescue aircraft (MMSI prefix 111)
    Sar,
}

/// Classify an AIS target by its MMSI number string (ITU-R M.585 prefix rules).
pub fn classify_mmsi(mmsi: &str) -> TargetClass {
    if mmsi.starts_with("97") {
        TargetClass::Aton
    } else if mmsi.starts_with("00") {
        TargetClass::Base
    } else if mmsi.starts_with("111") {
        TargetClass::Sar
    } else {
        TargetClass::Vessel
    }
}

/// Source-agnostic AIS contact data.
///
/// Both NMEA 0183 VDM and NMEA 2000 AIS PGN decoders fill this struct,
/// then call `to_delta()` to produce a SignalK delta with the correct
/// vessel context (`vessels.urn:mrn:imo:mmsi:XXXXXXXXX`).
///
/// All fields use **SI units**: m/s, radians, meters, decimal degrees.
#[derive(Debug, Clone, Default)]
pub struct AisContact {
    /// Maritime Mobile Service Identity (9 digits).
    pub mmsi: u32,
    /// Position as (latitude, longitude) in decimal degrees.
    pub position: Option<(f64, f64)>,
    /// Speed over ground in m/s.
    pub sog_ms: Option<f64>,
    /// Course over ground true in radians.
    pub cog_rad: Option<f64>,
    /// True heading in radians.
    pub heading_rad: Option<f64>,
    /// Rate of turn in rad/s.
    pub rot_rads: Option<f64>,
    /// ITU navigation status code (0–15).
    pub nav_status: Option<u8>,
    /// Vessel name.
    pub name: Option<String>,
    /// VHF callsign.
    pub callsign: Option<String>,
    /// IMO number.
    pub imo: Option<u32>,
    /// AIS ship type code (0–99).
    pub ship_type: Option<u8>,
    /// Destination port/area.
    pub destination: Option<String>,
    /// Maximum draught in meters.
    pub draught: Option<f64>,
    /// Overall length in meters (bow + stern).
    pub length: Option<f64>,
    /// Overall beam in meters (port + starboard).
    pub beam: Option<f64>,
}

impl AisContact {
    /// Vessel context string for SignalK deltas.
    pub fn context(&self) -> String {
        format!("vessels.urn:mrn:imo:mmsi:{:09}", self.mmsi)
    }

    /// Convert to a SignalK delta with all available fields.
    pub fn to_delta(&self, source: Source) -> Delta {
        let mut values = Vec::new();

        if let Some((lat, lon)) = self.position {
            values.push(PathValue::new(
                "navigation.position",
                serde_json::json!({"latitude": lat, "longitude": lon}),
            ));
        }

        if let Some(sog) = self.sog_ms {
            values.push(PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(sog),
            ));
        }

        if let Some(cog) = self.cog_rad {
            values.push(PathValue::new(
                "navigation.courseOverGroundTrue",
                serde_json::json!(cog),
            ));
        }

        if let Some(hdg) = self.heading_rad {
            values.push(PathValue::new(
                "navigation.headingTrue",
                serde_json::json!(hdg),
            ));
        }

        if let Some(rot) = self.rot_rads {
            values.push(PathValue::new(
                "navigation.rateOfTurn",
                serde_json::json!(rot),
            ));
        }

        if let Some(status) = self.nav_status
            && let Some(s) = nav_status_to_str(status)
        {
            values.push(PathValue::new("navigation.state", serde_json::json!(s)));
        }

        if let Some(ref name) = self.name
            && !name.trim().is_empty()
        {
            values.push(PathValue::new("name", serde_json::json!(name.trim())));
        }

        if let Some(ref cs) = self.callsign
            && !cs.trim().is_empty()
        {
            values.push(PathValue::new(
                "communication.callsignVhf",
                serde_json::json!(cs.trim()),
            ));
        }

        if let Some(imo) = self.imo
            && imo > 0
        {
            values.push(PathValue::new(
                "registrations.imo",
                serde_json::json!(format!("IMO {imo}")),
            ));
        }

        if let Some(st) = self.ship_type {
            values.push(PathValue::new(
                "design.aisShipType",
                serde_json::json!({"id": st, "name": ship_type_name(st)}),
            ));
        }

        if let Some(len) = self.length
            && len > 0.0
        {
            values.push(PathValue::new(
                "design.length.overall.value",
                serde_json::json!(len),
            ));
        }

        if let Some(beam) = self.beam
            && beam > 0.0
        {
            values.push(PathValue::new("design.beam.value", serde_json::json!(beam)));
        }

        if let Some(draft) = self.draught
            && draft > 0.0
        {
            values.push(PathValue::new(
                "design.draft.value.maximum",
                serde_json::json!(draft),
            ));
        }

        if let Some(ref dest) = self.destination
            && !dest.trim().is_empty()
        {
            values.push(PathValue::new(
                "navigation.destination.commonName",
                serde_json::json!(dest.trim()),
            ));
        }

        Delta::with_context(self.context(), vec![Update::new(source, values)])
    }
}

/// Map ITU navigation status code to SignalK state string.
fn nav_status_to_str(code: u8) -> Option<&'static str> {
    match code {
        0 => Some("motoring"),
        1 => Some("anchored"),
        2 => Some("not under command"),
        3 => Some("restricted maneuverability"),
        4 => Some("constrained by draft"),
        5 => Some("moored"),
        6 => Some("aground"),
        7 => Some("fishing"),
        8 => Some("sailing"),
        // 9-13 reserved
        14 => Some("ais-sart"),
        15 => None, // not defined / default
        _ => None,
    }
}

/// Map AIS ship type code to human-readable name.
fn ship_type_name(code: u8) -> &'static str {
    match code {
        0 => "Not available",
        1..=19 => "Reserved",
        20 => "Wing in ground",
        21..=29 => "Wing in ground",
        30 => "Fishing",
        31 => "Towing",
        32 => "Towing (large)",
        33 => "Dredging",
        34 => "Diving ops",
        35 => "Military ops",
        36 => "Sailing",
        37 => "Pleasure craft",
        38..=39 => "Reserved",
        40 => "High speed craft",
        41..=49 => "High speed craft",
        50 => "Pilot vessel",
        51 => "Search and rescue",
        52 => "Tug",
        53 => "Port tender",
        54 => "Anti-pollution",
        55 => "Law enforcement",
        56..=57 => "Spare",
        58 => "Medical transport",
        59 => "Non-combatant ship",
        60 => "Passenger",
        61..=69 => "Passenger",
        70 => "Cargo",
        71..=79 => "Cargo",
        80 => "Tanker",
        81..=89 => "Tanker",
        90 => "Other",
        91..=99 => "Other",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_contact() -> AisContact {
        AisContact {
            mmsi: 211457160,
            position: Some((54.123, 10.456)),
            sog_ms: Some(5.14444),
            cog_rad: Some(std::f64::consts::FRAC_PI_2),
            heading_rad: Some(std::f64::consts::FRAC_PI_2),
            rot_rads: Some(0.01),
            nav_status: Some(0),
            name: Some("PACIFIC EXPLORER".into()),
            callsign: Some("DJKL".into()),
            imo: Some(9876543),
            ship_type: Some(70),
            destination: Some("HAMBURG".into()),
            draught: Some(6.5),
            length: Some(120.0),
            beam: Some(18.0),
        }
    }

    #[test]
    fn context_format() {
        let c = AisContact {
            mmsi: 211457160,
            ..Default::default()
        };
        assert_eq!(c.context(), "vessels.urn:mrn:imo:mmsi:211457160");
    }

    #[test]
    fn context_zero_padded() {
        let c = AisContact {
            mmsi: 1234,
            ..Default::default()
        };
        assert_eq!(c.context(), "vessels.urn:mrn:imo:mmsi:000001234");
    }

    #[test]
    fn to_delta_full_contact() {
        let contact = full_contact();
        let delta = contact.to_delta(Source::nmea0183("ais-test", "AI"));

        assert_eq!(
            delta.context.as_deref(),
            Some("vessels.urn:mrn:imo:mmsi:211457160")
        );

        let values = &delta.updates[0].values;
        assert!(values.iter().any(|v| v.path == "navigation.position"));
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.speedOverGround")
        );
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.courseOverGroundTrue")
        );
        assert!(values.iter().any(|v| v.path == "navigation.headingTrue"));
        assert!(values.iter().any(|v| v.path == "navigation.rateOfTurn"));
        assert!(values.iter().any(|v| v.path == "navigation.state"));
        assert!(values.iter().any(|v| v.path == "name"));
        assert!(values.iter().any(|v| v.path == "communication.callsignVhf"));
        assert!(values.iter().any(|v| v.path == "registrations.imo"));
        assert!(values.iter().any(|v| v.path == "design.aisShipType"));
        assert!(
            values
                .iter()
                .any(|v| v.path == "design.length.overall.value")
        );
        assert!(values.iter().any(|v| v.path == "design.beam.value"));
        assert!(
            values
                .iter()
                .any(|v| v.path == "design.draft.value.maximum")
        );
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.destination.commonName")
        );
    }

    #[test]
    fn to_delta_minimal_position_report() {
        let contact = AisContact {
            mmsi: 366999000,
            position: Some((37.8, -122.4)),
            sog_ms: Some(2.5),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::nmea0183("test", "AI"));

        let values = &delta.updates[0].values;
        assert_eq!(values.len(), 2); // position + sog only
        assert_eq!(values[0].path, "navigation.position");
        assert_eq!(values[1].path, "navigation.speedOverGround");
    }

    #[test]
    fn to_delta_empty_fields_no_pathvalues() {
        let contact = AisContact {
            mmsi: 211000000,
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        assert!(delta.updates[0].values.is_empty());
    }

    #[test]
    fn position_values_correct() {
        let contact = AisContact {
            mmsi: 1,
            position: Some((54.123456, 10.987654)),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        let pos = &delta.updates[0].values[0].value;
        assert_eq!(pos["latitude"], 54.123456);
        assert_eq!(pos["longitude"], 10.987654);
    }

    #[test]
    fn sog_is_si_passthrough() {
        // AisContact takes m/s directly — no conversion in to_delta
        let contact = AisContact {
            mmsi: 1,
            sog_ms: Some(5.14444),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        assert_eq!(delta.updates[0].values[0].value, 5.14444);
    }

    #[test]
    fn nav_status_motoring() {
        let contact = AisContact {
            mmsi: 1,
            nav_status: Some(0),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        assert_eq!(delta.updates[0].values[0].value, "motoring");
    }

    #[test]
    fn nav_status_anchored() {
        let contact = AisContact {
            mmsi: 1,
            nav_status: Some(1),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        assert_eq!(delta.updates[0].values[0].value, "anchored");
    }

    #[test]
    fn nav_status_undefined_skipped() {
        let contact = AisContact {
            mmsi: 1,
            nav_status: Some(15), // default/not defined
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        assert!(delta.updates[0].values.is_empty());
    }

    #[test]
    fn ship_type_cargo() {
        let contact = AisContact {
            mmsi: 1,
            ship_type: Some(70),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        let st = &delta.updates[0].values[0].value;
        assert_eq!(st["id"], 70);
        assert_eq!(st["name"], "Cargo");
    }

    #[test]
    fn imo_formatted_correctly() {
        let contact = AisContact {
            mmsi: 1,
            imo: Some(9876543),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        assert_eq!(delta.updates[0].values[0].value, "IMO 9876543");
    }

    #[test]
    fn empty_name_skipped() {
        let contact = AisContact {
            mmsi: 1,
            name: Some("   ".into()),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        assert!(delta.updates[0].values.is_empty());
    }

    #[test]
    fn zero_imo_skipped() {
        let contact = AisContact {
            mmsi: 1,
            imo: Some(0),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        assert!(delta.updates[0].values.is_empty());
    }

    #[test]
    fn zero_length_skipped() {
        let contact = AisContact {
            mmsi: 1,
            length: Some(0.0),
            beam: Some(0.0),
            draught: Some(0.0),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::plugin("test"));
        assert!(delta.updates[0].values.is_empty());
    }

    #[test]
    fn source_preserved_in_delta() {
        let contact = AisContact {
            mmsi: 211000000,
            sog_ms: Some(3.0),
            ..Default::default()
        };
        let delta = contact.to_delta(Source::nmea0183("my-ais", "AI"));
        assert_eq!(delta.updates[0].source.type_, "NMEA0183");
        assert_eq!(delta.updates[0].source.label, "my-ais");
    }

    // ── MMSI Classification ─────────────────────────────────────────────

    #[test]
    fn classify_normal_vessel() {
        assert_eq!(classify_mmsi("211457160"), TargetClass::Vessel);
        assert_eq!(classify_mmsi("366999000"), TargetClass::Vessel);
    }

    #[test]
    fn classify_aton() {
        assert_eq!(classify_mmsi("970012345"), TargetClass::Aton);
        assert_eq!(classify_mmsi("979999999"), TargetClass::Aton);
    }

    #[test]
    fn classify_base_station() {
        assert_eq!(classify_mmsi("002111111"), TargetClass::Base);
        assert_eq!(classify_mmsi("003669999"), TargetClass::Base);
    }

    #[test]
    fn classify_sar() {
        assert_eq!(classify_mmsi("111123456"), TargetClass::Sar);
    }
}
