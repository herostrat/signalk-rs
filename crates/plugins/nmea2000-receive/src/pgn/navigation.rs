//! Navigation PGNs: VesselHeading, Speed, WaterDepth, PositionRapidUpdate,
//! CogSogRapidUpdate, GnssPositionData, RateOfTurn, Attitude,
//! DistanceLog, TimeDate, CrossTrackError
use super::{N2kSource, self_delta};
use nmea2000::pgns::*;
use signalk_types::{Delta, PathValue};

// ─── PGN 127250: Vessel Heading ──────────────────────────────────────────────

pub(super) fn from_vessel_heading(
    m: &vessel_heading::VesselHeading,
    source: &N2kSource<'_>,
) -> Option<Delta> {
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

// ─── PGN 128259: Speed, Water Referenced ─────────────────────────────────────

pub(super) fn from_speed(m: &speed::Speed, source: &N2kSource<'_>) -> Option<Delta> {
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

// ─── PGN 128267: Water Depth ──────────────────────────────────────────────────

pub(super) fn from_water_depth(
    m: &water_depth::WaterDepth,
    source: &N2kSource<'_>,
) -> Option<Delta> {
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

// ─── PGN 128275: Distance Log ─────────────────────────────────────────────────

pub(super) fn from_distance_log(
    m: &distance_log::DistanceLog,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mut values = Vec::new();

    if let Some(log) = m.log() {
        values.push(PathValue::new(
            "navigation.log",
            serde_json::json!(log as f64),
        ));
    }

    if let Some(trip) = m.trip_log() {
        values.push(PathValue::new(
            "navigation.trip.log",
            serde_json::json!(trip as f64),
        ));
    }

    self_delta(values, source)
}

// ─── PGN 129025: Position, Rapid Update ──────────────────────────────────────

pub(super) fn from_position_rapid(
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

// ─── PGN 129026: COG & SOG, Rapid Update ─────────────────────────────────────

pub(super) fn from_cog_sog_rapid(
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

// ─── PGN 129029: GNSS Position Data ──────────────────────────────────────────

pub(super) fn from_gnss_position(
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

// ─── PGN 129033: Time & Date ──────────────────────────────────────────────────

pub(super) fn from_time_date(m: &time_date::TimeDate, source: &N2kSource<'_>) -> Option<Delta> {
    let days = m.date()?;
    let secs = m.time()?;
    // NMEA 2000 epoch: days since 1970-01-01, seconds since midnight
    let total_secs = days as i64 * 86400 + secs as i64;
    let dt = chrono::DateTime::from_timestamp(total_secs, 0)?;
    let iso = dt.format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
    let total_ms = total_secs * 1000;
    self_delta(
        vec![
            PathValue::new("navigation.datetime", serde_json::json!(iso)),
            // N2K TimeDate is always UTC — timezoneOffset = 0, timezoneRegion = "UTC"
            PathValue::new("environment.time.millis", serde_json::json!(total_ms)),
            PathValue::new("environment.time.timezoneOffset", serde_json::json!(0)),
            PathValue::new("environment.time.timezoneRegion", serde_json::json!("UTC")),
        ],
        source,
    )
}

// ─── PGN 129283: Cross-Track Error ───────────────────────────────────────────

pub(super) fn from_cross_track_error(
    m: &cross_track_error::CrossTrackError,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let xte = m.xte()?;
    self_delta(
        vec![PathValue::new(
            "navigation.courseGreatCircle.crossTrackError",
            serde_json::json!(xte),
        )],
        source,
    )
}

// ─── PGN 127251: Rate of Turn ─────────────────────────────────────────────────

pub(super) fn from_rate_of_turn(
    m: &rate_of_turn::RateOfTurn,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let rate = m.rate()?;
    self_delta(
        vec![PathValue::new(
            "navigation.rateOfTurn",
            serde_json::json!(rate),
        )],
        source,
    )
}

// ─── PGN 127257: Attitude ─────────────────────────────────────────────────────

pub(super) fn from_attitude(m: &attitude::Attitude, source: &N2kSource<'_>) -> Option<Delta> {
    let mut values = Vec::new();

    if let Some(yaw) = m.yaw() {
        values.push(PathValue::new(
            "navigation.attitude.yaw",
            serde_json::json!(yaw),
        ));
    }
    if let Some(pitch) = m.pitch() {
        values.push(PathValue::new(
            "navigation.attitude.pitch",
            serde_json::json!(pitch),
        ));
    }
    if let Some(roll) = m.roll() {
        values.push(PathValue::new(
            "navigation.attitude.roll",
            serde_json::json!(roll),
        ));
    }

    self_delta(values, source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nmea2000::DecodedMessage;

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
            .heading(1.5)
            .reference_raw(0) // True
            .build();
        let decoded = DecodedMessage::VesselHeading(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(127250)).unwrap();
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
        let delta = super::super::decoded_to_delta(&decoded, &test_source(127250)).unwrap();
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
        let delta = super::super::decoded_to_delta(&decoded, &test_source(129025)).unwrap();
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
            .cog(1.0)
            .sog(5.0)
            .cog_reference_raw(0) // True
            .build();
        let decoded = DecodedMessage::CogSogRapidUpdate(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(129026)).unwrap();
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
            .offset(-1.5)
            .build();
        let decoded = DecodedMessage::WaterDepth(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(128267)).unwrap();
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
    fn rate_of_turn() {
        let msg = rate_of_turn::RateOfTurn::builder()
            .rate(0.1) // 0.1 rad/s
            .build();
        let decoded = DecodedMessage::RateOfTurn(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(127251)).unwrap();
        let values = &delta.updates[0].values;
        assert_eq!(values[0].path, "navigation.rateOfTurn");
        assert!((values[0].value.as_f64().unwrap() - 0.1).abs() < 1e-6);
    }

    #[test]
    fn attitude_roll_pitch_yaw() {
        let msg = attitude::Attitude::builder()
            .roll(0.05)
            .pitch(0.02)
            .yaw(1.5)
            .build();
        let decoded = DecodedMessage::Attitude(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(127257)).unwrap();
        let values = &delta.updates[0].values;
        assert!(values.iter().any(|v| v.path == "navigation.attitude.roll"));
        assert!(values.iter().any(|v| v.path == "navigation.attitude.pitch"));
        assert!(values.iter().any(|v| v.path == "navigation.attitude.yaw"));
    }

    #[test]
    fn distance_log() {
        let msg = distance_log::DistanceLog::builder()
            .log(1_000_000u64) // 1,000,000 m
            .trip_log(50_000u64) // 50,000 m
            .build();
        let decoded = DecodedMessage::DistanceLog(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(128275)).unwrap();
        let values = &delta.updates[0].values;
        let log = values.iter().find(|v| v.path == "navigation.log").unwrap();
        assert!((log.value.as_f64().unwrap() - 1_000_000.0).abs() < 1.0);
        let trip = values
            .iter()
            .find(|v| v.path == "navigation.trip.log")
            .unwrap();
        assert!((trip.value.as_f64().unwrap() - 50_000.0).abs() < 1.0);
    }

    #[test]
    fn time_date_iso8601() {
        // 2024-01-15 12:00:00 UTC: days = (2024-1970)*365 + leap adjustments
        // Use a known epoch: 2024-01-15 = 19737 days from 1970-01-01
        let msg = time_date::TimeDate::builder()
            .date(19737u16) // 2024-01-15
            .time(43200.0) // 12:00:00
            .build();
        let decoded = DecodedMessage::TimeDate(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(129033)).unwrap();
        let values = &delta.updates[0].values;
        assert_eq!(values.len(), 4);
        assert_eq!(values[0].path, "navigation.datetime");
        assert!(values[0].value.as_str().unwrap().contains("2024-01-15"));
    }

    #[test]
    fn time_date_writes_environment_time_paths() {
        // 2024-01-15 12:00:00 UTC = 19737 days + 43200 secs = total_secs 1705320000
        let msg = time_date::TimeDate::builder()
            .date(19737u16)
            .time(43200.0)
            .build();
        let decoded = DecodedMessage::TimeDate(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(129033)).unwrap();
        let values = &delta.updates[0].values;

        let millis = values
            .iter()
            .find(|p| p.path == "environment.time.millis")
            .unwrap();
        assert_eq!(millis.value.as_i64().unwrap(), 1705320000i64 * 1000);

        let offset = values
            .iter()
            .find(|p| p.path == "environment.time.timezoneOffset")
            .unwrap();
        assert_eq!(offset.value.as_i64().unwrap(), 0);

        let region = values
            .iter()
            .find(|p| p.path == "environment.time.timezoneRegion")
            .unwrap();
        assert_eq!(region.value.as_str().unwrap(), "UTC");
    }

    #[test]
    fn cross_track_error() {
        let msg = cross_track_error::CrossTrackError::builder()
            .xte(50.0) // 50 m XTE
            .build();
        let decoded = DecodedMessage::CrossTrackError(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(129283)).unwrap();
        let values = &delta.updates[0].values;
        assert_eq!(
            values[0].path,
            "navigation.courseGreatCircle.crossTrackError"
        );
        assert!((values[0].value.as_f64().unwrap() - 50.0).abs() < 0.1);
    }
}
