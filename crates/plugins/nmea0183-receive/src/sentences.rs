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
    pub(crate) fn new(path: impl Into<String>, value: Value) -> Self {
        PathValue {
            path: path.into(),
            value,
        }
    }
}

const KNOTS_TO_MS: f64 = 0.514_444;
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
const KELVIN_OFFSET: f64 = 273.15;
const BAR_TO_PA: f64 = 100_000.0;
const NM_TO_M: f64 = 1852.0;
const FEET_TO_M: f64 = 0.304_8;
const FATHOMS_TO_M: f64 = 1.828_8;

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

/// DBT — Depth Below Transducer
/// Provides: depth in meters (prefers meters field, falls back to feet/fathoms)
pub fn from_dbt(dbt: &nmea::sentences::DbtData) -> Vec<PathValue> {
    let mut out = Vec::new();
    let depth = dbt
        .depth_meters
        .or_else(|| dbt.depth_feet.map(|f| f * FEET_TO_M))
        .or_else(|| dbt.depth_fathoms.map(|f| f * FATHOMS_TO_M));
    if let Some(d) = depth {
        out.push(PathValue::new(
            "environment.depth.belowTransducer",
            json!(d),
        ));
    }
    out
}

/// DBS — Depth Below Surface
/// Provides: depth below surface in meters
pub fn from_dbs(dbs: &nmea::sentences::DbsData) -> Vec<PathValue> {
    let mut out = Vec::new();
    let depth = dbs
        .water_depth_meters
        .map(|m| m as f64)
        .or_else(|| dbs.water_depth_feet.map(|f| f as f64 * FEET_TO_M))
        .or_else(|| dbs.water_depth_fathoms.map(|f| f as f64 * FATHOMS_TO_M));
    if let Some(d) = depth {
        out.push(PathValue::new("environment.depth.belowSurface", json!(d)));
    }
    out
}

/// DBK — Depth Below Keel
/// Provides: depth below keel in meters
pub fn from_dbk(dbk: &nmea::sentences::DbkData) -> Vec<PathValue> {
    let mut out = Vec::new();
    let depth = dbk
        .depth_meters
        .or_else(|| dbk.depth_feet.map(|f| f * FEET_TO_M))
        .or_else(|| dbk.depth_fathoms.map(|f| f * FATHOMS_TO_M));
    if let Some(d) = depth {
        out.push(PathValue::new("environment.depth.belowKeel", json!(d)));
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

    // ─── New converter tests ───────────────────────────────────────────

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

    #[test]
    fn dbt_meters_preferred() {
        let dbt = nmea::sentences::DbtData {
            depth_feet: Some(49.2),
            depth_meters: Some(15.0),
            depth_fathoms: Some(8.2),
        };
        let values = from_dbt(&dbt);
        assert_eq!(values.len(), 1);
        assert!((values[0].value.as_f64().unwrap() - 15.0).abs() < 1e-6);
    }

    #[test]
    fn dbt_feet_fallback() {
        let dbt = nmea::sentences::DbtData {
            depth_feet: Some(49.2126),
            depth_meters: None,
            depth_fathoms: None,
        };
        let values = from_dbt(&dbt);
        assert!((values[0].value.as_f64().unwrap() - 49.2126 * FEET_TO_M).abs() < 1e-3);
    }

    #[test]
    fn dbs_depth_below_surface() {
        let dbs = nmea::sentences::DbsData {
            water_depth_feet: None,
            water_depth_meters: Some(20.0),
            water_depth_fathoms: None,
        };
        let values = from_dbs(&dbs);
        assert_eq!(values[0].path, "environment.depth.belowSurface");
        assert!((values[0].value.as_f64().unwrap() - 20.0).abs() < 1e-6);
    }

    #[test]
    fn dbk_depth_below_keel() {
        let dbk = nmea::sentences::DbkData {
            depth_feet: None,
            depth_meters: Some(3.5),
            depth_fathoms: None,
        };
        let values = from_dbk(&dbk);
        assert_eq!(values[0].path, "environment.depth.belowKeel");
        assert!((values[0].value.as_f64().unwrap() - 3.5).abs() < 1e-6);
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
