//! Heading sentences: HDT, HDM, HDG, VHW, ROT
use serde_json::json;
use super::PathValue;

const KNOTS_TO_MS: f64 = 0.514_444;
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// HDT — Heading, True
/// Provides: heading (true) in radians
pub fn from_hdt(hdt: &nmea::sentences::HdtData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if let Some(hdg) = hdt.heading {
        out.push(PathValue::new(
            "navigation.headingTrue",
            json!(hdg as f64 * DEG_TO_RAD),
        ));
    }
    out
}

/// HDG — Heading, Deviation & Variation
/// Provides: heading magnetic, magnetic deviation, magnetic variation
pub fn from_hdg(hdg: &nmea::sentences::HdgData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if let Some(h) = hdg.heading {
        out.push(PathValue::new(
            "navigation.headingMagnetic",
            json!(h as f64 * DEG_TO_RAD),
        ));
    }
    if let Some(d) = hdg.deviation {
        out.push(PathValue::new(
            "navigation.magneticDeviation",
            json!(d as f64 * DEG_TO_RAD),
        ));
    }
    if let Some(v) = hdg.variation {
        out.push(PathValue::new(
            "navigation.magneticVariation",
            json!(v as f64 * DEG_TO_RAD),
        ));
    }
    out
}

/// HDM — Heading, Magnetic
/// Provides: heading magnetic in radians
pub fn from_hdm(hdm: &nmea::sentences::HdmData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if let Some(h) = hdm.heading {
        out.push(PathValue::new(
            "navigation.headingMagnetic",
            json!(h as f64 * DEG_TO_RAD),
        ));
    }
    out
}

/// VHW — Water Speed and Heading
/// Provides: speed through water, heading true, heading magnetic
pub fn from_vhw(vhw: &nmea::sentences::VhwData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if let Some(stw) = vhw.relative_speed_knots {
        out.push(PathValue::new(
            "navigation.speedThroughWater",
            json!(stw * KNOTS_TO_MS),
        ));
    }
    if let Some(h) = vhw.heading_true {
        out.push(PathValue::new(
            "navigation.headingTrue",
            json!(h * DEG_TO_RAD),
        ));
    }
    if let Some(h) = vhw.heading_magnetic {
        out.push(PathValue::new(
            "navigation.headingMagnetic",
            json!(h * DEG_TO_RAD),
        ));
    }
    out
}

/// ROT — Rate of Turn
/// Provides: rate of turn in rad/s (NMEA gives deg/min)
pub fn from_rot(rot: &nmea::sentences::RotData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if rot.valid != Some(false)
        && let Some(r) = rot.rate
    {
        out.push(PathValue::new(
            "navigation.rateOfTurn",
            json!(r as f64 * DEG_TO_RAD / 60.0),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdt_heading_to_radians() {
        let hdt = nmea::sentences::HdtData {
            heading: Some(180.0),
        };
        let values = from_hdt(&hdt);
        let hdg = values
            .iter()
            .find(|p| p.path == "navigation.headingTrue")
            .unwrap();
        let v = hdg.value.as_f64().unwrap();
        assert!(
            (v - std::f64::consts::PI).abs() < 1e-6,
            "Expected PI, got {v}"
        );
    }

    #[test]
    fn hdg_all_fields() {
        let hdg = nmea::sentences::HdgData {
            heading: Some(90.0),
            deviation: Some(2.0),
            variation: Some(-3.5),
        };
        let values = from_hdg(&hdg);
        assert_eq!(values.len(), 3);
        let h = values
            .iter()
            .find(|p| p.path == "navigation.headingMagnetic")
            .unwrap();
        assert!((h.value.as_f64().unwrap() - 90.0 * DEG_TO_RAD).abs() < 1e-6);
        let d = values
            .iter()
            .find(|p| p.path == "navigation.magneticDeviation")
            .unwrap();
        assert!((d.value.as_f64().unwrap() - 2.0 * DEG_TO_RAD).abs() < 1e-6);
        let v = values
            .iter()
            .find(|p| p.path == "navigation.magneticVariation")
            .unwrap();
        assert!((v.value.as_f64().unwrap() - (-3.5) * DEG_TO_RAD).abs() < 1e-6);
    }

    #[test]
    fn hdm_heading_magnetic() {
        let hdm = nmea::sentences::HdmData {
            heading: Some(270.0),
        };
        let values = from_hdm(&hdm);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].path, "navigation.headingMagnetic");
        assert!((values[0].value.as_f64().unwrap() - 270.0 * DEG_TO_RAD).abs() < 1e-6);
    }

    #[test]
    fn vhw_speed_and_heading() {
        let vhw = nmea::sentences::VhwData {
            heading_true: Some(180.0),
            heading_magnetic: Some(177.0),
            relative_speed_knots: Some(6.5),
            relative_speed_kmph: None,
        };
        let values = from_vhw(&vhw);
        let stw = values
            .iter()
            .find(|p| p.path == "navigation.speedThroughWater")
            .unwrap();
        assert!((stw.value.as_f64().unwrap() - 6.5 * KNOTS_TO_MS).abs() < 1e-6);
        let ht = values
            .iter()
            .find(|p| p.path == "navigation.headingTrue")
            .unwrap();
        assert!((ht.value.as_f64().unwrap() - std::f64::consts::PI).abs() < 1e-6);
        let hm = values
            .iter()
            .find(|p| p.path == "navigation.headingMagnetic")
            .unwrap();
        assert!((hm.value.as_f64().unwrap() - 177.0 * DEG_TO_RAD).abs() < 1e-6);
    }

    #[test]
    fn rot_deg_per_min_to_rad_per_sec() {
        let rot = nmea::sentences::RotData {
            rate: Some(30.0), // 30 deg/min
            valid: Some(true),
        };
        let values = from_rot(&rot);
        assert_eq!(values.len(), 1);
        let expected = 30.0 * DEG_TO_RAD / 60.0;
        assert!((values[0].value.as_f64().unwrap() - expected).abs() < 1e-8);
    }

    #[test]
    fn rot_invalid_ignored() {
        let rot = nmea::sentences::RotData {
            rate: Some(30.0),
            valid: Some(false),
        };
        assert!(from_rot(&rot).is_empty());
    }
}
