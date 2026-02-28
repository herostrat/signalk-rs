/// NMEA 0183 sentence → SignalK Delta conversion.
///
/// Each function takes a parsed NMEA sentence and returns a list of
/// (path, value) pairs for inclusion in a Delta update.
///
/// Unit conversions follow the SignalK specification:
///   - Speed  : knots     → m/s   (× 0.514 444)
///   - Angles : degrees   → radians
///   - Depth  : meters    (already SI)
///   - Lat/Lon: decimal degrees (no radian conversion — SK spec uses degrees for position)
use serde_json::{Value, json};

/// One SignalK path-value pair extracted from a sentence.
pub struct PathValue {
    pub path: String,
    pub value: Value,
}

impl PathValue {
    fn new(path: impl Into<String>, value: Value) -> Self {
        PathValue {
            path: path.into(),
            value,
        }
    }
}

const KNOTS_TO_MS: f64 = 0.514_444;
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// RMC — Recommended Minimum Specific GNSS Data
/// Provides: position, speed over ground, course over ground, magnetic variation
pub fn from_rmc(rmc: &nmea::sentences::RmcData) -> Vec<PathValue> {
    let mut out = Vec::new();

    if let (Some(lat), Some(lon)) = (rmc.lat, rmc.lon) {
        out.push(PathValue::new(
            "navigation.position",
            json!({ "latitude": lat, "longitude": lon }),
        ));
    }

    if let Some(sog) = rmc.speed_over_ground {
        out.push(PathValue::new(
            "navigation.speedOverGround",
            json!(sog as f64 * KNOTS_TO_MS),
        ));
    }

    if let Some(cog) = rmc.true_course {
        out.push(PathValue::new(
            "navigation.courseOverGroundTrue",
            json!(cog as f64 * DEG_TO_RAD),
        ));
    }

    // magnetic_variation is already signed: negative = West, positive = East
    if let Some(variation) = rmc.magnetic_variation {
        out.push(PathValue::new(
            "navigation.magneticVariation",
            json!(variation as f64 * DEG_TO_RAD),
        ));
    }

    out
}

/// GGA — Global Positioning System Fix Data
/// Provides: position (lat/lon/altitude), fix quality, HDOP, satellites
pub fn from_gga(gga: &nmea::sentences::GgaData) -> Vec<PathValue> {
    let mut out = Vec::new();

    if let (Some(lat), Some(lon)) = (gga.latitude, gga.longitude) {
        let mut pos = json!({ "latitude": lat, "longitude": lon });
        if let Some(alt) = gga.altitude {
            pos["altitude"] = json!(alt);
        }
        out.push(PathValue::new("navigation.position", pos));
    }

    if let Some(hdop) = gga.hdop {
        out.push(PathValue::new(
            "navigation.gnss.horizontalDilution",
            json!(hdop),
        ));
    }

    if let Some(sats) = gga.fix_satellites {
        out.push(PathValue::new("navigation.gnss.satellites", json!(sats)));
    }

    if let Some(fix) = &gga.fix_type {
        use nmea::sentences::FixType;
        let method = match fix {
            FixType::Invalid => "no GPS",
            FixType::Gps => "GNSS",
            FixType::DGps => "DGNSS",
            FixType::Pps => "PPS",
            FixType::Rtk => "RTK fixed",
            FixType::FloatRtk => "RTK float",
            FixType::Estimated => "estimated",
            FixType::Manual => "manual",
            FixType::Simulation => "simulation",
        };
        out.push(PathValue::new(
            "navigation.gnss.methodQuality",
            json!(method),
        ));
    }

    out
}

/// VTG — Course and Speed Over Ground
/// Provides: COG (true), SOG
/// Note: `speed_over_ground` from the nmea crate is in knots.
pub fn from_vtg(vtg: &nmea::sentences::VtgData) -> Vec<PathValue> {
    let mut out = Vec::new();

    if let Some(cog_true) = vtg.true_course {
        out.push(PathValue::new(
            "navigation.courseOverGroundTrue",
            json!(cog_true as f64 * DEG_TO_RAD),
        ));
    }

    if let Some(sog_knots) = vtg.speed_over_ground {
        out.push(PathValue::new(
            "navigation.speedOverGround",
            json!(sog_knots as f64 * KNOTS_TO_MS),
        ));
    }

    out
}

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

/// DPT — Depth of Water
/// Provides: depth below transducer; optionally below keel or surface
pub fn from_dpt(dpt: &nmea::sentences::DptData) -> Vec<PathValue> {
    let mut out = Vec::new();

    let Some(depth) = dpt.water_depth else {
        return out;
    };

    out.push(PathValue::new(
        "environment.depth.belowTransducer",
        json!(depth),
    ));

    // offset: positive → transducer above waterline (depth + offset = depth below surface)
    //         negative → transducer above keel       (depth + offset = depth below keel)
    if let Some(offset) = dpt.offset {
        let adjusted = depth + offset;
        if offset >= 0.0 {
            out.push(PathValue::new(
                "environment.depth.belowSurface",
                json!(adjusted),
            ));
        } else {
            out.push(PathValue::new(
                "environment.depth.belowKeel",
                json!(adjusted),
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rmc_speed_converted_to_ms() {
        use nmea::sentences::rmc::RmcStatusOfFix;
        let rmc = nmea::sentences::RmcData {
            fix_time: None,
            fix_date: None,
            status_of_fix: RmcStatusOfFix::Autonomous,
            lat: Some(53.0),
            lon: Some(9.0),
            speed_over_ground: Some(1.0),
            true_course: None,
            magnetic_variation: None,
            faa_mode: None,
            nav_status: None,
        };
        let values = from_rmc(&rmc);
        let sog = values
            .iter()
            .find(|p| p.path == "navigation.speedOverGround")
            .unwrap();
        let v = sog.value.as_f64().unwrap();
        assert!(
            (v - KNOTS_TO_MS).abs() < 1e-6,
            "Expected ~0.514444, got {v}"
        );
    }

    #[test]
    fn rmc_position_extracted() {
        use nmea::sentences::rmc::RmcStatusOfFix;
        let rmc = nmea::sentences::RmcData {
            fix_time: None,
            fix_date: None,
            status_of_fix: RmcStatusOfFix::Autonomous,
            lat: Some(53.5),
            lon: Some(9.9),
            speed_over_ground: None,
            true_course: None,
            magnetic_variation: None,
            faa_mode: None,
            nav_status: None,
        };
        let values = from_rmc(&rmc);
        let pos = values
            .iter()
            .find(|p| p.path == "navigation.position")
            .unwrap();
        assert_eq!(pos.value["latitude"], 53.5);
        assert_eq!(pos.value["longitude"], 9.9);
    }

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
    fn dpt_below_transducer() {
        let dpt = nmea::sentences::DptData {
            water_depth: Some(15.3),
            offset: None,
            max_range_scale: None,
        };
        let values = from_dpt(&dpt);
        let depth = values
            .iter()
            .find(|p| p.path == "environment.depth.belowTransducer")
            .unwrap();
        assert!((depth.value.as_f64().unwrap() - 15.3).abs() < 1e-6);
    }

    #[test]
    fn dpt_below_keel_from_negative_offset() {
        let dpt = nmea::sentences::DptData {
            water_depth: Some(15.3),
            offset: Some(-1.5), // transducer is 1.5 m above keel
            max_range_scale: None,
        };
        let values = from_dpt(&dpt);
        let keel = values
            .iter()
            .find(|p| p.path == "environment.depth.belowKeel")
            .unwrap();
        assert!((keel.value.as_f64().unwrap() - 13.8).abs() < 1e-6);
    }

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
    fn vtg_cog_to_radians() {
        let vtg = nmea::sentences::VtgData {
            true_course: Some(90.0),
            speed_over_ground: None,
        };
        let values = from_vtg(&vtg);
        let cog = values
            .iter()
            .find(|p| p.path == "navigation.courseOverGroundTrue")
            .unwrap();
        let v = cog.value.as_f64().unwrap();
        assert!(
            (v - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "Expected π/2, got {v}"
        );
    }
}
