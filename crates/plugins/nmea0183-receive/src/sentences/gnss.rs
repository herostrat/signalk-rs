//! GNSS sentences: GNS, GSA, ZDA
use super::PathValue;
use serde_json::json;

/// ZDA — Time & Date
/// Provides: UTC date+time as ISO 8601 string → navigation.datetime,
///           environment.time.millis, environment.time.timezoneOffset/Region
pub fn from_zda(zda: &nmea::sentences::ZdaData) -> Vec<PathValue> {
    use chrono::{NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};

    let mut out = Vec::new();
    if let (Some(t), Some(day), Some(month), Some(year)) =
        (zda.utc_time, zda.day, zda.month, zda.year)
    {
        let ms = t.nanosecond() / 1_000_000;
        let iso = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            year,
            month,
            day,
            t.hour(),
            t.minute(),
            t.second(),
            ms
        );
        out.push(PathValue::new("navigation.datetime", json!(iso)));

        // Compute Unix timestamp in milliseconds (GPS is always UTC)
        if let Some(date) = NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32) {
            let ndt = NaiveDateTime::new(date, t);
            let utc = Utc.from_utc_datetime(&ndt);
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

/// GSA — GNSS DOP and Active Satellites
/// Provides: HDOP, PDOP, satellite count (only when fix satellites are listed)
pub fn from_gsa(gsa: &nmea::sentences::GsaData) -> Vec<PathValue> {
    let mut out = Vec::new();

    if let Some(hdop) = gsa.hdop {
        out.push(PathValue::new(
            "navigation.gnss.horizontalDilution",
            json!(hdop as f64),
        ));
    }
    if let Some(pdop) = gsa.pdop {
        out.push(PathValue::new(
            "navigation.gnss.positionDilution",
            json!(pdop as f64),
        ));
    }
    let sats = gsa.fix_sats_prn.len() as u64;
    if sats > 0 {
        out.push(PathValue::new("navigation.gnss.satellites", json!(sats)));
    }
    out
}

/// GNS — GNSS Fix Data (multi-constellation)
/// Provides: position, satellite count, HDOP, fix method quality
pub fn from_gns(gns: &nmea::sentences::GnsData) -> Vec<PathValue> {
    let mut out = Vec::new();

    if let (Some(lat), Some(lon)) = (gns.lat, gns.lon) {
        let mut pos = json!({ "latitude": lat, "longitude": lon });
        if let Some(alt) = gns.alt {
            pos["altitude"] = json!(alt);
        }
        out.push(PathValue::new("navigation.position", pos));
    }

    if let Some(hdop) = gns.hdop {
        out.push(PathValue::new(
            "navigation.gnss.horizontalDilution",
            json!(hdop as f64),
        ));
    }

    out.push(PathValue::new(
        "navigation.gnss.satellites",
        json!(gns.nsattelites as u64),
    ));

    if let Some(status) = &gns.nav_status {
        use nmea::sentences::gns::NavigationStatus;
        let method = match status {
            NavigationStatus::Safe | NavigationStatus::Caution => "GNSS",
            NavigationStatus::Unsafe => "estimated",
            NavigationStatus::NotValidForNavigation => "no GPS",
        };
        out.push(PathValue::new(
            "navigation.gnss.methodQuality",
            json!(method),
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zda_builds_iso8601() {
        use chrono::NaiveTime;
        let zda = nmea::sentences::ZdaData {
            utc_time: NaiveTime::from_hms_milli_opt(12, 34, 56, 789),
            day: Some(15),
            month: Some(6),
            year: Some(2024),
            local_zone_hours: None,
            local_zone_minutes: None,
        };
        let values = from_zda(&zda);
        assert_eq!(values.len(), 4); // datetime + millis + timezoneOffset + timezoneRegion
        let dt = values
            .iter()
            .find(|p| p.path == "navigation.datetime")
            .unwrap();
        assert_eq!(dt.value.as_str().unwrap(), "2024-06-15T12:34:56.789Z");
    }

    #[test]
    fn zda_writes_environment_time_paths() {
        use chrono::NaiveTime;
        let zda = nmea::sentences::ZdaData {
            utc_time: NaiveTime::from_hms_milli_opt(12, 0, 0, 0),
            day: Some(15),
            month: Some(6),
            year: Some(2024),
            local_zone_hours: None,
            local_zone_minutes: None,
        };
        let values = from_zda(&zda);

        let millis = values
            .iter()
            .find(|p| p.path == "environment.time.millis")
            .unwrap();
        assert!(millis.value.as_i64().unwrap() > 0);
        // 2024-06-15T12:00:00Z in ms = 1718452800000
        assert_eq!(millis.value.as_i64().unwrap(), 1718452800000_i64);

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
    fn zda_missing_date_emits_nothing() {
        use chrono::NaiveTime;
        let zda = nmea::sentences::ZdaData {
            utc_time: NaiveTime::from_hms_opt(10, 0, 0),
            day: None,
            month: Some(1),
            year: Some(2024),
            local_zone_hours: None,
            local_zone_minutes: None,
        };
        assert!(from_zda(&zda).is_empty());
    }

    #[test]
    fn gsa_hdop_and_pdop() {
        let gsa = nmea::sentences::GsaData {
            mode1: nmea::sentences::gsa::GsaMode1::Automatic,
            mode2: nmea::sentences::gsa::GsaMode2::Fix3D,
            fix_sats_prn: Default::default(),
            pdop: Some(1.8),
            hdop: Some(1.2),
            vdop: Some(1.5),
            system_id: None,
        };
        let values = from_gsa(&gsa);
        let hdop = values
            .iter()
            .find(|p| p.path == "navigation.gnss.horizontalDilution")
            .unwrap();
        assert!((hdop.value.as_f64().unwrap() - 1.2).abs() < 1e-5);
        let pdop = values
            .iter()
            .find(|p| p.path == "navigation.gnss.positionDilution")
            .unwrap();
        assert!((pdop.value.as_f64().unwrap() - 1.8).abs() < 1e-5);
        // No satellites (empty fix_sats_prn)
        assert!(
            values
                .iter()
                .all(|p| p.path != "navigation.gnss.satellites")
        );
    }

    #[test]
    fn gns_position_and_satellites() {
        // FaaModes has private fields; parse a real sentence to get GnsData.
        let raw = "$GPGNS,224749.00,3333.4268304,N,11153.3538273,W,D,19,0.6,406.110,-26.294,6.0,0138,S,*46";
        let parsed = nmea::parse_str(raw).unwrap();
        let nmea::ParseResult::GNS(gns) = parsed else {
            panic!("expected GNS");
        };
        let values = from_gns(&gns);

        let pos = values
            .iter()
            .find(|p| p.path == "navigation.position")
            .unwrap();
        assert!(pos.value["latitude"].as_f64().unwrap().abs() > 0.0);

        let sats = values
            .iter()
            .find(|p| p.path == "navigation.gnss.satellites")
            .unwrap();
        assert_eq!(sats.value.as_u64().unwrap(), 19);

        let method = values
            .iter()
            .find(|p| p.path == "navigation.gnss.methodQuality")
            .unwrap();
        // nav_status = S (Safe) → "GNSS"
        assert_eq!(method.value.as_str().unwrap(), "GNSS");
    }
}
