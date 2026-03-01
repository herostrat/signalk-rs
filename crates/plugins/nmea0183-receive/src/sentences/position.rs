//! Position sentences: RMC, GGA, GLL, VTG
use super::PathValue;
use serde_json::json;

const KNOTS_TO_MS: f64 = 0.514_444;
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// RMC — Recommended Minimum Specific GNSS Data
/// Provides: position, speed over ground, course over ground, magnetic variation,
///           navigation.datetime, environment.time.millis/timezoneOffset/timezoneRegion
pub fn from_rmc(rmc: &nmea::sentences::RmcData) -> Vec<PathValue> {
    use chrono::{NaiveDateTime, TimeZone, Utc};
    use nmea::sentences::rmc::RmcStatusOfFix;

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

    // Time paths: only when fix is valid (Autonomous or Differential) and full date+time available.
    // GPS always transmits UTC — timezoneOffset = 0, timezoneRegion = "UTC".
    let fix_valid = matches!(
        rmc.status_of_fix,
        RmcStatusOfFix::Autonomous | RmcStatusOfFix::Differential
    );
    if fix_valid {
        if let (Some(date), Some(time)) = (rmc.fix_date, rmc.fix_time) {
            let ndt = NaiveDateTime::new(date, time);
            let utc = Utc.from_utc_datetime(&ndt);
            let iso = utc.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            out.push(PathValue::new("navigation.datetime", json!(iso)));
            out.push(PathValue::new(
                "environment.time.millis",
                json!(utc.timestamp_millis()),
            ));
            out.push(PathValue::new("environment.time.timezoneOffset", json!(0)));
            out.push(PathValue::new(
                "environment.time.timezoneRegion",
                json!("UTC"),
            ));
        }
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
            FixType::Sbas => "DGNSS",
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

/// GLL — Geographic Position
/// Provides: position (lat/lon)
pub fn from_gll(gll: &nmea::sentences::GllData) -> Vec<PathValue> {
    let mut out = Vec::new();
    if !gll.valid {
        return out;
    }
    if let (Some(lat), Some(lon)) = (gll.latitude, gll.longitude) {
        out.push(PathValue::new(
            "navigation.position",
            json!({ "latitude": lat, "longitude": lon }),
        ));
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
    fn rmc_valid_fix_writes_datetime_and_time_paths() {
        use chrono::{NaiveDate, NaiveTime};
        use nmea::sentences::rmc::RmcStatusOfFix;
        let rmc = nmea::sentences::RmcData {
            fix_time: NaiveTime::from_hms_milli_opt(12, 0, 0, 0),
            fix_date: NaiveDate::from_ymd_opt(2024, 6, 15),
            status_of_fix: RmcStatusOfFix::Autonomous,
            lat: None,
            lon: None,
            speed_over_ground: None,
            true_course: None,
            magnetic_variation: None,
            faa_mode: None,
            nav_status: None,
        };
        let values = from_rmc(&rmc);

        let dt = values
            .iter()
            .find(|p| p.path == "navigation.datetime")
            .unwrap();
        assert_eq!(dt.value.as_str().unwrap(), "2024-06-15T12:00:00.000Z");

        let millis = values
            .iter()
            .find(|p| p.path == "environment.time.millis")
            .unwrap();
        assert!(millis.value.as_i64().unwrap() > 0);

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
    fn rmc_invalid_fix_does_not_write_time_paths() {
        use chrono::{NaiveDate, NaiveTime};
        use nmea::sentences::rmc::RmcStatusOfFix;
        let rmc = nmea::sentences::RmcData {
            fix_time: NaiveTime::from_hms_opt(12, 0, 0),
            fix_date: NaiveDate::from_ymd_opt(2024, 6, 15),
            status_of_fix: RmcStatusOfFix::Invalid,
            lat: None,
            lon: None,
            speed_over_ground: None,
            true_course: None,
            magnetic_variation: None,
            faa_mode: None,
            nav_status: None,
        };
        let values = from_rmc(&rmc);
        assert!(!values.iter().any(|p| p.path == "navigation.datetime"));
        assert!(!values.iter().any(|p| p.path == "environment.time.millis"));
    }

    #[test]
    fn rmc_missing_date_does_not_write_time_paths() {
        use chrono::NaiveTime;
        use nmea::sentences::rmc::RmcStatusOfFix;
        let rmc = nmea::sentences::RmcData {
            fix_time: NaiveTime::from_hms_opt(12, 0, 0),
            fix_date: None, // no date
            status_of_fix: RmcStatusOfFix::Autonomous,
            lat: None,
            lon: None,
            speed_over_ground: None,
            true_course: None,
            magnetic_variation: None,
            faa_mode: None,
            nav_status: None,
        };
        let values = from_rmc(&rmc);
        assert!(!values.iter().any(|p| p.path == "navigation.datetime"));
        assert!(!values.iter().any(|p| p.path == "environment.time.millis"));
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

    #[test]
    fn gll_position() {
        let gll = nmea::sentences::GllData {
            latitude: Some(54.5),
            longitude: Some(10.0),
            fix_time: None,
            valid: true,
            faa_mode: None,
        };
        let values = from_gll(&gll);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].value["latitude"], 54.5);
        assert_eq!(values[0].value["longitude"], 10.0);
    }

    #[test]
    fn gll_invalid_ignored() {
        let gll = nmea::sentences::GllData {
            latitude: Some(54.5),
            longitude: Some(10.0),
            fix_time: None,
            valid: false,
            faa_mode: None,
        };
        assert!(from_gll(&gll).is_empty());
    }
}
