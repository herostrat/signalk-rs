//! Propulsion sentences: RSA, RPM
use serde_json::json;
use super::PathValue;

const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// RSA — Rudder Sensor Angle
/// Provides: rudder angle (positive = starboard)
pub fn from_rsa(rsa: &nmea::sentences::RsaData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if rsa.starboard_valid
        && let Some(a) = rsa.starboard
    {
        out.push(PathValue::new(
            "steering.rudderAngle",
            json!(a as f64 * DEG_TO_RAD),
        ));
    }
    if rsa.port_valid
        && let Some(a) = rsa.port
    {
        out.push(PathValue::new(
            "steering.rudderAnglePort",
            json!(a as f64 * DEG_TO_RAD),
        ));
    }
    out
}

/// RPM — Revolutions
/// Provides: engine/shaft revolutions (rev/s) and propeller pitch (%)
pub fn from_rpm(rpm: &nmea::sentences::RpmData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if !rpm.valid {
        return out;
    }

    let source_num = rpm.source_number.unwrap_or(0);
    let prefix = match rpm.source {
        Some(nmea::sentences::rpm::RpmSource::Engine) => {
            format!("propulsion.engine{source_num}")
        }
        Some(nmea::sentences::rpm::RpmSource::Shaft) | None => {
            format!("propulsion.shaft{source_num}")
        }
    };

    if let Some(r) = rpm.rpm {
        out.push(PathValue::new(
            format!("{prefix}.revolutions"),
            json!(r as f64 / 60.0),
        ));
    }
    if let Some(p) = rpm.pitch {
        out.push(PathValue::new(
            format!("{prefix}.pitch"),
            json!(p as f64 / 100.0),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsa_starboard_and_port() {
        let rsa = nmea::sentences::RsaData {
            starboard: Some(5.0),
            starboard_valid: true,
            port: Some(-3.0),
            port_valid: true,
        };
        let values = from_rsa(&rsa);
        assert_eq!(values.len(), 2);
        let sb = values
            .iter()
            .find(|p| p.path == "steering.rudderAngle")
            .unwrap();
        assert!((sb.value.as_f64().unwrap() - 5.0 * DEG_TO_RAD).abs() < 1e-6);
        let pt = values
            .iter()
            .find(|p| p.path == "steering.rudderAnglePort")
            .unwrap();
        assert!((pt.value.as_f64().unwrap() - (-3.0) * DEG_TO_RAD).abs() < 1e-6);
    }

    #[test]
    fn rsa_invalid_ignored() {
        let rsa = nmea::sentences::RsaData {
            starboard: Some(5.0),
            starboard_valid: false,
            port: None,
            port_valid: false,
        };
        assert!(from_rsa(&rsa).is_empty());
    }

    #[test]
    fn rpm_engine_revolutions() {
        let rpm = nmea::sentences::RpmData {
            source: Some(nmea::sentences::rpm::RpmSource::Engine),
            source_number: Some(1),
            rpm: Some(2400.0),
            pitch: Some(75.0),
            valid: true,
        };
        let values = from_rpm(&rpm);
        let rev = values
            .iter()
            .find(|p| p.path == "propulsion.engine1.revolutions")
            .unwrap();
        assert!((rev.value.as_f64().unwrap() - 40.0).abs() < 1e-6); // 2400/60
        let pitch = values
            .iter()
            .find(|p| p.path == "propulsion.engine1.pitch")
            .unwrap();
        assert!((pitch.value.as_f64().unwrap() - 0.75).abs() < 1e-6); // 75/100
    }

    #[test]
    fn rpm_invalid_ignored() {
        let rpm = nmea::sentences::RpmData {
            source: Some(nmea::sentences::rpm::RpmSource::Engine),
            source_number: Some(0),
            rpm: Some(1200.0),
            pitch: None,
            valid: false,
        };
        assert!(from_rpm(&rpm).is_empty());
    }
}
