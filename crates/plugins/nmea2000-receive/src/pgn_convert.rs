//! PGN → SignalK Delta conversion.
//!
//! Dispatches decoded NMEA 2000 messages to SignalK delta producers.
//! Navigation PGNs produce self-vessel deltas; AIS PGNs produce
//! vessel-context deltas via `AisContact::to_delta()`.

use nmea2000::DecodedMessage;
use nmea2000::pgns::*;
use signalk_types::ais::AisContact;
use signalk_types::{Delta, PathValue, Source, Update};

/// Source info from the raw NMEA 2000 message.
pub struct N2kSource<'a> {
    pub label: &'a str,
    pub src: u8,
    pub pgn: u32,
}

/// Convert a decoded NMEA 2000 message to a SignalK delta.
///
/// Returns `None` for unsupported PGNs or messages with no usable data.
pub fn decoded_to_delta(msg: &DecodedMessage, source: &N2kSource<'_>) -> Option<Delta> {
    match msg {
        // ── Navigation PGNs (self vessel) ──────────────────────────────
        DecodedMessage::VesselHeading(m) => from_vessel_heading(m, source),
        DecodedMessage::Speed(m) => from_speed(m, source),
        DecodedMessage::WaterDepth(m) => from_water_depth(m, source),
        DecodedMessage::PositionRapidUpdate(m) => from_position_rapid(m, source),
        DecodedMessage::CogSogRapidUpdate(m) => from_cog_sog_rapid(m, source),
        DecodedMessage::GnssPositionData(m) => from_gnss_position(m, source),
        DecodedMessage::WindData(m) => from_wind_data(m, source),

        // ── AIS PGNs (vessel context by MMSI) ─────────────────────────
        DecodedMessage::AisClassAPositionReport(m) => from_ais_class_a_position(m, source),
        DecodedMessage::AisClassBPositionReport(m) => from_ais_class_b_position(m, source),
        DecodedMessage::AisClassBExtendedPositionReport(m) => from_ais_class_b_extended(m, source),
        DecodedMessage::AisClassAStaticAndVoyageRelatedData(m) => {
            from_ais_class_a_static(m, source)
        }
        DecodedMessage::AisClassBStaticDataMsg24PartA(m) => from_ais_static_24a(m, source),
        DecodedMessage::AisClassBStaticDataMsg24PartB(m) => from_ais_static_24b(m, source),

        _ => None,
    }
}

// ─── Navigation PGN helpers ──────────────────────────────────────────────────

fn make_source(s: &N2kSource<'_>) -> Source {
    Source::nmea2000(s.label, s.src, s.pgn)
}

fn self_delta(values: Vec<PathValue>, source: &N2kSource<'_>) -> Option<Delta> {
    if values.is_empty() {
        return None;
    }
    Some(Delta::self_vessel(vec![Update::new(
        make_source(source),
        values,
    )]))
}

/// PGN 127250: Vessel Heading
fn from_vessel_heading(m: &vessel_heading::VesselHeading, source: &N2kSource<'_>) -> Option<Delta> {
    let mut values = Vec::new();

    if let Some(heading) = m.heading() {
        let path = match m.reference() {
            Some(lookups::DirectionReference::True) => "navigation.headingTrue",
            Some(lookups::DirectionReference::Magnetic) => "navigation.headingMagnetic",
            _ => "navigation.headingTrue",
        };
        values.push(PathValue::new(path, serde_json::json!(heading)));
    }

    if let Some(variation) = m.variation() {
        values.push(PathValue::new(
            "navigation.magneticVariation",
            serde_json::json!(variation),
        ));
    }

    self_delta(values, source)
}

/// PGN 128259: Speed, Water Referenced
fn from_speed(m: &speed::Speed, source: &N2kSource<'_>) -> Option<Delta> {
    let mut values = Vec::new();

    if let Some(stw) = m.speed_water_referenced() {
        values.push(PathValue::new(
            "navigation.speedThroughWater",
            serde_json::json!(stw),
        ));
    }

    if let Some(sog) = m.speed_ground_referenced() {
        values.push(PathValue::new(
            "navigation.speedOverGround",
            serde_json::json!(sog),
        ));
    }

    self_delta(values, source)
}

/// PGN 128267: Water Depth
fn from_water_depth(m: &water_depth::WaterDepth, source: &N2kSource<'_>) -> Option<Delta> {
    let mut values = Vec::new();

    if let Some(depth) = m.depth() {
        values.push(PathValue::new(
            "environment.depth.belowTransducer",
            serde_json::json!(depth),
        ));

        if let Some(offset) = m.offset() {
            let adjusted = depth + offset;
            if offset >= 0.0 {
                values.push(PathValue::new(
                    "environment.depth.belowSurface",
                    serde_json::json!(adjusted),
                ));
            } else {
                values.push(PathValue::new(
                    "environment.depth.belowKeel",
                    serde_json::json!(adjusted),
                ));
            }
        }
    }

    self_delta(values, source)
}

/// PGN 129025: Position, Rapid Update
fn from_position_rapid(
    m: &position_rapid_update::PositionRapidUpdate,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let (lat, lon) = (m.latitude()?, m.longitude()?);
    self_delta(
        vec![PathValue::new(
            "navigation.position",
            serde_json::json!({"latitude": lat, "longitude": lon}),
        )],
        source,
    )
}

/// PGN 129026: COG & SOG, Rapid Update
fn from_cog_sog_rapid(
    m: &cog_sog_rapid_update::CogSogRapidUpdate,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mut values = Vec::new();

    if let Some(cog) = m.cog() {
        let path = match m.cog_reference() {
            Some(lookups::DirectionReference::True) => "navigation.courseOverGroundTrue",
            Some(lookups::DirectionReference::Magnetic) => "navigation.courseOverGroundMagnetic",
            _ => "navigation.courseOverGroundTrue",
        };
        values.push(PathValue::new(path, serde_json::json!(cog)));
    }

    if let Some(sog) = m.sog() {
        values.push(PathValue::new(
            "navigation.speedOverGround",
            serde_json::json!(sog),
        ));
    }

    self_delta(values, source)
}

/// PGN 129029: GNSS Position Data
fn from_gnss_position(
    m: &gnss_position_data::GnssPositionData,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mut values = Vec::new();

    if let (Some(lat), Some(lon)) = (m.latitude(), m.longitude()) {
        let mut pos = serde_json::json!({"latitude": lat, "longitude": lon});
        if let Some(alt) = m.altitude() {
            pos["altitude"] = serde_json::json!(alt);
        }
        values.push(PathValue::new("navigation.position", pos));
    }

    self_delta(values, source)
}

/// PGN 130306: Wind Data
fn from_wind_data(m: &wind_data::WindData, source: &N2kSource<'_>) -> Option<Delta> {
    let mut values = Vec::new();

    let (speed_path, angle_path) = match m.reference() {
        Some(lookups::WindReference::Apparent) => (
            "environment.wind.speedApparent",
            "environment.wind.angleApparent",
        ),
        Some(lookups::WindReference::TrueBoatReferenced)
        | Some(lookups::WindReference::TrueWaterReferenced) => (
            "environment.wind.speedTrue",
            "environment.wind.angleTrueWater",
        ),
        Some(lookups::WindReference::TrueGroundReferencedToNorth) => (
            "environment.wind.speedTrue",
            "environment.wind.angleTrueGround",
        ),
        _ => return None,
    };

    if let Some(speed) = m.wind_speed() {
        values.push(PathValue::new(speed_path, serde_json::json!(speed)));
    }

    if let Some(angle) = m.wind_angle() {
        values.push(PathValue::new(angle_path, serde_json::json!(angle)));
    }

    self_delta(values, source)
}

// ─── AIS PGN helpers ─────────────────────────────────────────────────────────

/// PGN 129038: AIS Class A Position Report
fn from_ais_class_a_position(
    m: &ais_class_a_position_report::AisClassAPositionReport,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        position: match (m.latitude(), m.longitude()) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        },
        sog_ms: m.sog(),
        cog_rad: m.cog(),
        heading_rad: m.heading(),
        rot_rads: m.rate_of_turn(),
        nav_status: m.nav_status_raw().map(|v| v as u8),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

/// PGN 129039: AIS Class B Position Report
fn from_ais_class_b_position(
    m: &ais_class_b_position_report::AisClassBPositionReport,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        position: match (m.latitude(), m.longitude()) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        },
        sog_ms: m.sog(),
        cog_rad: m.cog(),
        heading_rad: m.heading(),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

/// PGN 129040: AIS Class B Extended Position Report
fn from_ais_class_b_extended(
    m: &ais_class_b_extended_position_report::AisClassBExtendedPositionReport,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        position: match (m.latitude(), m.longitude()) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        },
        sog_ms: m.sog(),
        cog_rad: m.cog(),
        heading_rad: m.true_heading(),
        name: m.name().map(str::to_string),
        ship_type: m.type_of_ship_raw().map(|v| v as u8),
        length: m.length().filter(|&v| v > 0.0),
        beam: m.beam().filter(|&v| v > 0.0),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

/// PGN 129794: AIS Class A Static and Voyage Related Data
fn from_ais_class_a_static(
    m: &ais_class_a_static_and_voyage_related_data::AisClassAStaticAndVoyageRelatedData,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        name: m.name().map(str::to_string),
        callsign: m.callsign().map(str::to_string),
        imo: m.imo_number().map(|v| v as u32),
        ship_type: m.type_of_ship_raw().map(|v| v as u8),
        destination: m.destination().map(str::to_string),
        draught: m.draft(),
        length: m.length().filter(|&v| v > 0.0),
        beam: m.beam().filter(|&v| v > 0.0),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

/// PGN 129809: AIS Class B Static Data, Part A (name)
fn from_ais_static_24a(
    m: &ais_class_b_static_data_msg24_part_a::AisClassBStaticDataMsg24PartA,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        name: m.name().map(str::to_string),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

/// PGN 129810: AIS Class B Static Data, Part B (callsign, type, dimensions)
fn from_ais_static_24b(
    m: &ais_class_b_static_data_msg24_part_b::AisClassBStaticDataMsg24PartB,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        callsign: m.callsign().map(str::to_string),
        ship_type: m.type_of_ship_raw().map(|v| v as u8),
        length: m.length().filter(|&v| v > 0.0),
        beam: m.beam().filter(|&v| v > 0.0),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_source(pgn: u32) -> N2kSource<'static> {
        N2kSource {
            label: "n2k",
            src: 0,
            pgn,
        }
    }

    #[test]
    fn vessel_heading_true() {
        let msg = vessel_heading::VesselHeading::builder()
            .heading(1.5) // 1.5 rad
            .reference_raw(0) // True
            .build();
        let decoded = DecodedMessage::VesselHeading(msg);
        let delta = decoded_to_delta(&decoded, &test_source(127250)).unwrap();
        assert_eq!(delta.context.as_deref(), Some("vessels.self"));
        let values = &delta.updates[0].values;
        assert_eq!(values[0].path, "navigation.headingTrue");
        let hdg = values[0].value.as_f64().unwrap();
        assert!((hdg - 1.5).abs() < 0.001);
    }

    #[test]
    fn vessel_heading_magnetic() {
        let msg = vessel_heading::VesselHeading::builder()
            .heading(2.0)
            .reference_raw(1) // Magnetic
            .variation(0.05)
            .build();
        let decoded = DecodedMessage::VesselHeading(msg);
        let delta = decoded_to_delta(&decoded, &test_source(127250)).unwrap();
        let values = &delta.updates[0].values;
        assert_eq!(values[0].path, "navigation.headingMagnetic");
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.magneticVariation")
        );
    }

    #[test]
    fn position_rapid_update() {
        let msg = position_rapid_update::PositionRapidUpdate::builder()
            .latitude(54.123)
            .longitude(10.456)
            .build();
        let decoded = DecodedMessage::PositionRapidUpdate(msg);
        let delta = decoded_to_delta(&decoded, &test_source(129025)).unwrap();
        let pos = &delta.updates[0].values[0];
        assert_eq!(pos.path, "navigation.position");
        let lat = pos.value["latitude"].as_f64().unwrap();
        let lon = pos.value["longitude"].as_f64().unwrap();
        assert!((lat - 54.123).abs() < 1e-5);
        assert!((lon - 10.456).abs() < 1e-5);
    }

    #[test]
    fn cog_sog_rapid() {
        let msg = cog_sog_rapid_update::CogSogRapidUpdate::builder()
            .cog(1.0) // 1.0 rad
            .sog(5.0) // 5.0 m/s
            .cog_reference_raw(0) // True
            .build();
        let decoded = DecodedMessage::CogSogRapidUpdate(msg);
        let delta = decoded_to_delta(&decoded, &test_source(129026)).unwrap();
        let values = &delta.updates[0].values;
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.courseOverGroundTrue")
        );
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.speedOverGround")
        );
    }

    #[test]
    fn water_depth_with_offset() {
        let msg = water_depth::WaterDepth::builder()
            .depth(15.0)
            .offset(-1.5) // below keel
            .build();
        let decoded = DecodedMessage::WaterDepth(msg);
        let delta = decoded_to_delta(&decoded, &test_source(128267)).unwrap();
        let values = &delta.updates[0].values;
        assert!(
            values
                .iter()
                .any(|v| v.path == "environment.depth.belowTransducer")
        );
        assert!(
            values
                .iter()
                .any(|v| v.path == "environment.depth.belowKeel")
        );
    }

    #[test]
    fn wind_data_apparent() {
        let msg = wind_data::WindData::builder()
            .wind_speed(8.0) // 8 m/s
            .wind_angle(0.785) // ~45°
            .reference_raw(2) // Apparent
            .build();
        let decoded = DecodedMessage::WindData(msg);
        let delta = decoded_to_delta(&decoded, &test_source(130306)).unwrap();
        let values = &delta.updates[0].values;
        assert!(
            values
                .iter()
                .any(|v| v.path == "environment.wind.speedApparent")
        );
        assert!(
            values
                .iter()
                .any(|v| v.path == "environment.wind.angleApparent")
        );
    }

    #[test]
    fn ais_class_a_position_report() {
        let msg = ais_class_a_position_report::AisClassAPositionReport::builder()
            .user_id_raw(211457160)
            .latitude(54.3)
            .longitude(10.5)
            .sog(5.0)
            .cog(1.0)
            .heading(1.1)
            .nav_status_raw(0) // Under way using engine
            .build();
        let decoded = DecodedMessage::AisClassAPositionReport(msg);
        let delta = decoded_to_delta(&decoded, &test_source(129038)).unwrap();
        assert!(
            delta
                .context
                .as_deref()
                .unwrap()
                .starts_with("vessels.urn:mrn:imo:mmsi:211457160")
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
        assert!(values.iter().any(|v| v.path == "navigation.state"));
    }

    #[test]
    fn ais_class_b_position_report() {
        let msg = ais_class_b_position_report::AisClassBPositionReport::builder()
            .user_id_raw(366999000)
            .latitude(37.8)
            .longitude(-122.4)
            .sog(2.5)
            .build();
        let decoded = DecodedMessage::AisClassBPositionReport(msg);
        let delta = decoded_to_delta(&decoded, &test_source(129039)).unwrap();
        assert!(delta.context.as_deref().unwrap().contains("366999000"));
        let values = &delta.updates[0].values;
        assert!(values.iter().any(|v| v.path == "navigation.position"));
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.speedOverGround")
        );
    }

    #[test]
    fn source_is_nmea2000() {
        let msg = position_rapid_update::PositionRapidUpdate::builder()
            .latitude(54.0)
            .longitude(10.0)
            .build();
        let decoded = DecodedMessage::PositionRapidUpdate(msg);
        let src = N2kSource {
            label: "my-n2k",
            src: 42,
            pgn: 129025,
        };
        let delta = decoded_to_delta(&decoded, &src).unwrap();
        assert_eq!(delta.updates[0].source.type_, "NMEA2000");
        assert_eq!(delta.updates[0].source.label, "my-n2k");
    }

    #[test]
    fn unsupported_pgn_returns_none() {
        let msg = iso_request::IsoRequest::builder().build();
        let decoded = DecodedMessage::IsoRequest(msg);
        assert!(decoded_to_delta(&decoded, &test_source(59904)).is_none());
    }
}
