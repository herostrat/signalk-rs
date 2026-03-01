//! Wind sentences: MWV, MWD, VWR, VWT
use serde_json::json;
use super::PathValue;

const KNOTS_TO_MS: f64 = 0.514_444;
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// MWV — Wind Speed and Angle
/// Provides: apparent or true wind speed + angle
///
/// nmea crate mapping:
///   `MwvReference::Relative`     → apparent wind (relative to boat)
///   `MwvReference::Theoretical`  → true wind (relative to water/ground)
pub fn from_mwv(mwv: &nmea::sentences::MwvData) -> Vec<PathValue> {
    let mut out = Vec::new();

    if !mwv.data_valid {
        return out;
    }

    let angle_rad = mwv.wind_direction.map(|a| a as f64 * DEG_TO_RAD);

    let speed_ms = mwv.wind_speed.and_then(|s| {
        use nmea::sentences::mwv::MwvWindSpeedUnits;
        match mwv.wind_speed_units {
            Some(MwvWindSpeedUnits::Knots) => Some(s as f64 * KNOTS_TO_MS),
            Some(MwvWindSpeedUnits::MetersPerSecond) => Some(s as f64),
            Some(MwvWindSpeedUnits::KilometersPerHour) => Some(s as f64 / 3.6),
            Some(MwvWindSpeedUnits::MilesPerHour) => Some(s as f64 * 0.447_04),
            None => None,
        }
    });

    use nmea::sentences::mwv::MwvReference;
    match mwv.reference {
        Some(MwvReference::Relative) => {
            if let Some(angle) = angle_rad {
                out.push(PathValue::new(
                    "environment.wind.angleApparent",
                    json!(angle),
                ));
            }
            if let Some(speed) = speed_ms {
                out.push(PathValue::new(
                    "environment.wind.speedApparent",
                    json!(speed),
                ));
            }
        }
        Some(MwvReference::Theoretical) => {
            if let Some(angle) = angle_rad {
                out.push(PathValue::new(
                    "environment.wind.angleTrueWater",
                    json!(angle),
                ));
            }
            if let Some(speed) = speed_ms {
                out.push(PathValue::new("environment.wind.speedTrue", json!(speed)));
            }
        }
        None => {}
    }

    out
}

/// MWD — Wind Direction & Speed
/// Provides: true/magnetic wind direction, true wind speed
pub fn from_mwd(mwd: &nmea::sentences::MwdData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if let Some(dir) = mwd.wind_direction_true {
        out.push(PathValue::new(
            "environment.wind.directionTrue",
            json!(dir as f64 * DEG_TO_RAD),
        ));
    }
    if let Some(dir) = mwd.wind_direction_magnetic {
        out.push(PathValue::new(
            "environment.wind.directionMagnetic",
            json!(dir as f64 * DEG_TO_RAD),
        ));
    }
    if let Some(spd) = mwd.wind_speed_mps {
        out.push(PathValue::new(
            "environment.wind.speedTrue",
            json!(spd as f64),
        ));
    } else if let Some(spd) = mwd.wind_speed_knots {
        out.push(PathValue::new(
            "environment.wind.speedTrue",
            json!(spd as f64 * KNOTS_TO_MS),
        ));
    }
    out
}

/// VWR — Relative (Apparent) Wind Speed and Angle
/// Provides: apparent wind angle + speed
/// Angle: 0-180, negative for port, positive for starboard
pub fn from_vwr(vwr: &nmea::sentences::VwrData) -> Vec<PathValue> {
    let mut out = Vec::new();
    // wind_angle is already signed: negative = port, positive = starboard
    if let Some(a) = vwr.wind_angle {
        out.push(PathValue::new(
            "environment.wind.angleApparent",
            json!(a as f64 * DEG_TO_RAD),
        ));
    }
    let speed = vwr
        .speed_mps
        .map(|s| s as f64)
        .or_else(|| vwr.speed_knots.map(|s| s as f64 * KNOTS_TO_MS));
    if let Some(s) = speed {
        out.push(PathValue::new("environment.wind.speedApparent", json!(s)));
    }
    out
}

/// VWT — True Wind Speed and Angle
/// Provides: true wind angle + speed relative to water
pub fn from_vwt(vwt: &nmea::sentences::VwtData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if let Some(a) = vwt.wind_angle {
        out.push(PathValue::new(
            "environment.wind.angleTrueWater",
            json!(a as f64 * DEG_TO_RAD),
        ));
    }
    let speed = vwt
        .speed_mps
        .map(|s| s as f64)
        .or_else(|| vwt.speed_knots.map(|s| s as f64 * KNOTS_TO_MS));
    if let Some(s) = speed {
        out.push(PathValue::new("environment.wind.speedTrue", json!(s)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mwv_apparent_knots_to_ms() {
        let mwv = nmea::sentences::MwvData {
            wind_direction: Some(45.0),
            reference: Some(nmea::sentences::mwv::MwvReference::Relative),
            wind_speed: Some(10.0),
            wind_speed_units: Some(nmea::sentences::mwv::MwvWindSpeedUnits::Knots),
            data_valid: true,
        };
        let values = from_mwv(&mwv);
        let speed = values
            .iter()
            .find(|p| p.path == "environment.wind.speedApparent")
            .unwrap();
        let v = speed.value.as_f64().unwrap();
        assert!((v - 10.0 * KNOTS_TO_MS).abs() < 1e-6);
    }

    #[test]
    fn mwv_invalid_ignored() {
        let mwv = nmea::sentences::MwvData {
            wind_direction: Some(45.0),
            reference: Some(nmea::sentences::mwv::MwvReference::Relative),
            wind_speed: Some(10.0),
            wind_speed_units: Some(nmea::sentences::mwv::MwvWindSpeedUnits::Knots),
            data_valid: false,
        };
        assert!(from_mwv(&mwv).is_empty());
    }

    #[test]
    fn mwd_wind_direction_and_speed() {
        let mwd = nmea::sentences::MwdData {
            wind_direction_true: Some(270.0),
            wind_direction_magnetic: Some(267.0),
            wind_speed_knots: Some(15.0),
            wind_speed_mps: Some(7.72),
        };
        let values = from_mwd(&mwd);
        let dt = values
            .iter()
            .find(|p| p.path == "environment.wind.directionTrue")
            .unwrap();
        assert!((dt.value.as_f64().unwrap() - 270.0 * DEG_TO_RAD).abs() < 1e-6);
        let dm = values
            .iter()
            .find(|p| p.path == "environment.wind.directionMagnetic")
            .unwrap();
        assert!((dm.value.as_f64().unwrap() - 267.0 * DEG_TO_RAD).abs() < 1e-6);
        // Prefers m/s over knots
        let spd = values
            .iter()
            .find(|p| p.path == "environment.wind.speedTrue")
            .unwrap();
        assert!((spd.value.as_f64().unwrap() - 7.72).abs() < 1e-6);
    }

    #[test]
    fn vwr_apparent_wind() {
        let vwr = nmea::sentences::VwrData {
            wind_angle: Some(-45.0), // port
            speed_knots: Some(10.0),
            speed_mps: Some(5.14),
            speed_kmph: None,
        };
        let values = from_vwr(&vwr);
        let angle = values
            .iter()
            .find(|p| p.path == "environment.wind.angleApparent")
            .unwrap();
        assert!((angle.value.as_f64().unwrap() - (-45.0) * DEG_TO_RAD).abs() < 1e-6);
        // Prefers m/s
        let spd = values
            .iter()
            .find(|p| p.path == "environment.wind.speedApparent")
            .unwrap();
        assert!((spd.value.as_f64().unwrap() - 5.14).abs() < 1e-6);
    }

    #[test]
    fn vwt_true_wind() {
        let vwt = nmea::sentences::VwtData {
            wind_angle: Some(120.0),
            speed_knots: Some(20.0),
            speed_mps: None,
            speed_kmph: None,
        };
        let values = from_vwt(&vwt);
        let angle = values
            .iter()
            .find(|p| p.path == "environment.wind.angleTrueWater")
            .unwrap();
        assert!((angle.value.as_f64().unwrap() - 120.0 * DEG_TO_RAD).abs() < 1e-6);
        let spd = values
            .iter()
            .find(|p| p.path == "environment.wind.speedTrue")
            .unwrap();
        assert!((spd.value.as_f64().unwrap() - 20.0 * KNOTS_TO_MS).abs() < 1e-6);
    }
}
