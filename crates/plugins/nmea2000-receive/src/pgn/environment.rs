//! Environment PGNs: WindData (130306), Temperature (130312)
use nmea2000::pgns::*;
use signalk_types::{Delta, PathValue};
use super::{N2kSource, self_delta};

// ─── PGN 130306: Wind Data ────────────────────────────────────────────────────

pub(super) fn from_wind_data(m: &wind_data::WindData, source: &N2kSource<'_>) -> Option<Delta> {
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

// ─── PGN 130312: Temperature ──────────────────────────────────────────────────

pub(super) fn from_temperature(
    m: &temperature::Temperature,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let temp = m.actual_temperature()?;
    let path = match m.source()? {
        lookups::TemperatureSource::SeaTemperature => "environment.water.temperature",
        lookups::TemperatureSource::OutsideTemperature => "environment.outside.temperature",
        lookups::TemperatureSource::InsideTemperature => "environment.inside.temperature",
        lookups::TemperatureSource::MainCabinTemperature => {
            "environment.inside.mainCabinTemperature"
        }
        _ => return None,
    };
    self_delta(vec![PathValue::new(path, serde_json::json!(temp))], source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nmea2000::DecodedMessage;

    fn test_source(pgn: u32) -> N2kSource<'static> {
        N2kSource { label: "n2k", src: 0, pgn }
    }

    #[test]
    fn wind_data_apparent() {
        let msg = wind_data::WindData::builder()
            .wind_speed(8.0)
            .wind_angle(0.785)
            .reference_raw(2) // Apparent
            .build();
        let decoded = DecodedMessage::WindData(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(130306)).unwrap();
        let values = &delta.updates[0].values;
        assert!(values.iter().any(|v| v.path == "environment.wind.speedApparent"));
        assert!(values.iter().any(|v| v.path == "environment.wind.angleApparent"));
    }

    #[test]
    fn temperature_sea() {
        let msg = temperature::Temperature::builder()
            .actual_temperature(293.15) // 20°C
            .source_raw(0) // SeaTemperature
            .build();
        let decoded = DecodedMessage::Temperature(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(130312)).unwrap();
        let values = &delta.updates[0].values;
        assert_eq!(values[0].path, "environment.water.temperature");
        assert!((values[0].value.as_f64().unwrap() - 293.15).abs() < 0.01);
    }

    #[test]
    fn temperature_outside() {
        let msg = temperature::Temperature::builder()
            .actual_temperature(288.15) // 15°C
            .source_raw(1) // OutsideTemperature
            .build();
        let decoded = DecodedMessage::Temperature(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(130312)).unwrap();
        let values = &delta.updates[0].values;
        assert_eq!(values[0].path, "environment.outside.temperature");
    }
}
