//! Environment sentences: MTW, MDA, VDR, VLW
use super::PathValue;
use serde_json::json;

const KNOTS_TO_MS: f64 = 0.514_444;
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
const KELVIN_OFFSET: f64 = 273.15;
const BAR_TO_PA: f64 = 100_000.0;
const NM_TO_M: f64 = 1852.0;

/// MTW — Mean Temperature of Water
/// Provides: water temperature in Kelvin (SignalK spec)
pub fn from_mtw(mtw: &nmea::sentences::MtwData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if let Some(t) = mtw.temperature {
        out.push(PathValue::new(
            "environment.water.temperature",
            json!(t + KELVIN_OFFSET),
        ));
    }
    out
}

/// MDA — Meteorological Composite
/// Provides: barometric pressure, air/water temp, humidity, dew point, wind
pub fn from_mda(mda: &nmea::sentences::MdaData) -> Vec<PathValue> {
    let mut out = Vec::new();

    if let Some(p) = mda.pressure_bar {
        out.push(PathValue::new(
            "environment.outside.pressure",
            json!(p as f64 * BAR_TO_PA),
        ));
    }
    if let Some(t) = mda.air_temp_deg {
        out.push(PathValue::new(
            "environment.outside.temperature",
            json!(t as f64 + KELVIN_OFFSET),
        ));
    }
    if let Some(t) = mda.water_temp_deg {
        out.push(PathValue::new(
            "environment.water.temperature",
            json!(t as f64 + KELVIN_OFFSET),
        ));
    }
    if let Some(h) = mda.rel_humidity {
        out.push(PathValue::new(
            "environment.outside.humidity",
            json!(h as f64 / 100.0),
        ));
    }
    if let Some(d) = mda.dew_point {
        out.push(PathValue::new(
            "environment.outside.dewPointTemperature",
            json!(d as f64 + KELVIN_OFFSET),
        ));
    }
    if let Some(dir) = mda.wind_direction_true {
        out.push(PathValue::new(
            "environment.wind.directionTrue",
            json!(dir as f64 * DEG_TO_RAD),
        ));
    }
    if let Some(spd) = mda.wind_speed_ms {
        out.push(PathValue::new(
            "environment.wind.speedTrue",
            json!(spd as f64),
        ));
    } else if let Some(spd) = mda.wind_speed_knots {
        out.push(PathValue::new(
            "environment.wind.speedTrue",
            json!(spd as f64 * KNOTS_TO_MS),
        ));
    }

    out
}

/// VDR — Set and Drift
/// Provides: current set (direction) and drift (speed)
pub fn from_vdr(vdr: &nmea::sentences::VdrData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if let Some(set) = vdr.direction_true {
        out.push(PathValue::new(
            "environment.current.setTrue",
            json!(set as f64 * DEG_TO_RAD),
        ));
    }
    if let Some(drift) = vdr.speed {
        out.push(PathValue::new(
            "environment.current.drift",
            json!(drift as f64 * KNOTS_TO_MS),
        ));
    }
    out
}

/// VLW — Distance Traveled through Water
/// Provides: trip log, total log (NM → meters)
pub fn from_vlw(vlw: &nmea::sentences::VlwData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if let Some(trip) = vlw.trip_water_distance {
        out.push(PathValue::new(
            "navigation.trip.log",
            json!(trip as f64 * NM_TO_M),
        ));
    }
    if let Some(total) = vlw.total_water_distance {
        out.push(PathValue::new(
            "navigation.log",
            json!(total as f64 * NM_TO_M),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mtw_celsius_to_kelvin() {
        let mtw = nmea::sentences::MtwData {
            temperature: Some(22.5),
        };
        let values = from_mtw(&mtw);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].path, "environment.water.temperature");
        assert!((values[0].value.as_f64().unwrap() - 295.65).abs() < 1e-6);
    }

    #[test]
    fn mda_pressure_and_temp() {
        let mda = nmea::sentences::MdaData {
            pressure_in_hg: None,
            pressure_bar: Some(1.013),
            air_temp_deg: Some(20.0),
            water_temp_deg: Some(15.0),
            rel_humidity: Some(65.0),
            abs_humidity: None,
            dew_point: Some(13.0),
            wind_direction_true: Some(180.0),
            wind_direction_magnetic: None,
            wind_speed_knots: None,
            wind_speed_ms: Some(5.0),
        };
        let values = from_mda(&mda);
        let p = values
            .iter()
            .find(|p| p.path == "environment.outside.pressure")
            .unwrap();
        assert!((p.value.as_f64().unwrap() - 101_300.0).abs() < 1.0);
        let air = values
            .iter()
            .find(|p| p.path == "environment.outside.temperature")
            .unwrap();
        assert!((air.value.as_f64().unwrap() - 293.15).abs() < 1e-6);
        let water = values
            .iter()
            .find(|p| p.path == "environment.water.temperature")
            .unwrap();
        assert!((water.value.as_f64().unwrap() - 288.15).abs() < 1e-6);
        let hum = values
            .iter()
            .find(|p| p.path == "environment.outside.humidity")
            .unwrap();
        assert!((hum.value.as_f64().unwrap() - 0.65).abs() < 1e-6);
        let dew = values
            .iter()
            .find(|p| p.path == "environment.outside.dewPointTemperature")
            .unwrap();
        assert!((dew.value.as_f64().unwrap() - 286.15).abs() < 1e-6);
        let wdir = values
            .iter()
            .find(|p| p.path == "environment.wind.directionTrue")
            .unwrap();
        assert!((wdir.value.as_f64().unwrap() - std::f64::consts::PI).abs() < 1e-6);
        let wspd = values
            .iter()
            .find(|p| p.path == "environment.wind.speedTrue")
            .unwrap();
        assert!((wspd.value.as_f64().unwrap() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn vdr_set_and_drift() {
        let vdr = nmea::sentences::VdrData {
            direction_true: Some(180.0),
            direction_magnetic: None,
            speed: Some(1.5),
        };
        let values = from_vdr(&vdr);
        let set = values
            .iter()
            .find(|p| p.path == "environment.current.setTrue")
            .unwrap();
        assert!((set.value.as_f64().unwrap() - std::f64::consts::PI).abs() < 1e-6);
        let drift = values
            .iter()
            .find(|p| p.path == "environment.current.drift")
            .unwrap();
        assert!((drift.value.as_f64().unwrap() - 1.5 * KNOTS_TO_MS).abs() < 1e-6);
    }

    #[test]
    fn vlw_distance_nm_to_meters() {
        let vlw = nmea::sentences::VlwData {
            total_water_distance: Some(1234.5),
            trip_water_distance: Some(56.7),
            total_ground_distance: None,
            trip_ground_distance: None,
        };
        let values = from_vlw(&vlw);
        let trip = values
            .iter()
            .find(|p| p.path == "navigation.trip.log")
            .unwrap();
        assert!((trip.value.as_f64().unwrap() - 56.7 * NM_TO_M).abs() < 1.0);
        let total = values.iter().find(|p| p.path == "navigation.log").unwrap();
        assert!((total.value.as_f64().unwrap() - 1234.5 * NM_TO_M).abs() < 1.0);
    }
}
