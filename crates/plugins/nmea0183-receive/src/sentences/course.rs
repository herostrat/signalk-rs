//! Course sentences: RMB, BWC, BOD, XTE
use super::PathValue;
use serde_json::json;

const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
const NM_TO_M: f64 = 1852.0;

/// RMB — Recommended Minimum Navigation Information
/// Provides: destination position, range, bearing, cross-track error
/// Only emits values when sentence status is active (not void).
pub fn from_rmb(rmb: &nmea::sentences::RmbData) -> Vec<PathValue> {
    if !rmb.status {
        return vec![];
    }

    let mut out = Vec::new();

    if let (Some(lat), Some(lon)) = (rmb.dest_latitude, rmb.dest_longitude) {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.nextPoint.position",
            json!({ "latitude": lat, "longitude": lon }),
        ));
    }

    if let Some(nm) = rmb.range_to_dest {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.nextPoint.distance",
            json!(nm as f64 * NM_TO_M),
        ));
    }

    if let Some(deg) = rmb.bearing_to_dest {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.nextPoint.bearingTrue",
            json!(deg as f64 * DEG_TO_RAD),
        ));
    }

    if let Some(xte_nm) = rmb.cross_track_error {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.crossTrackError",
            json!(xte_nm as f64 * NM_TO_M),
        ));
    }

    out
}

/// BWC — Bearing & Distance to Waypoint — Great Circle
/// Provides: next waypoint position, bearing (true/magnetic), distance
pub fn from_bwc(bwc: &nmea::sentences::BwcData) -> Vec<PathValue> {
    let mut out = Vec::new();

    if let (Some(lat), Some(lon)) = (bwc.latitude, bwc.longitude) {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.nextPoint.position",
            json!({ "latitude": lat, "longitude": lon }),
        ));
    }

    if let Some(b) = bwc.true_bearing {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.nextPoint.bearingTrue",
            json!(b as f64 * DEG_TO_RAD),
        ));
    }

    if let Some(b) = bwc.magnetic_bearing {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.nextPoint.bearingMagnetic",
            json!(b as f64 * DEG_TO_RAD),
        ));
    }

    if let Some(d) = bwc.distance {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.nextPoint.distance",
            json!(d as f64 * NM_TO_M),
        ));
    }

    out
}

/// BOD — Bearing, Origin to Destination
/// Provides: bearing true and magnetic from origin to destination waypoint
pub fn from_bod(bod: &nmea::sentences::BodData) -> Vec<PathValue> {
    let mut out = Vec::new();

    if let Some(b) = bod.bearing_true {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.bearingTrackTrue",
            json!(b as f64 * DEG_TO_RAD),
        ));
    }

    if let Some(b) = bod.bearing_magnetic {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.bearingTrackMagnetic",
            json!(b as f64 * DEG_TO_RAD),
        ));
    }

    out
}

/// XTE — Cross-Track Error, Measured
/// Provides: signed cross-track error in meters.
/// Positive = vessel right of track (steer left to correct).
/// Returns empty when status_general is false (fault/warning condition).
pub fn from_xte(xte: &nmea::sentences::XteData) -> Vec<PathValue> {
    if !xte.status_general {
        return vec![];
    }

    let mut out = Vec::new();
    if let Some(err) = xte.cross_track_error {
        out.push(PathValue::new(
            "navigation.courseGreatCircle.crossTrackError",
            json!(err as f64 * NM_TO_M),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rmb_active() -> nmea::sentences::RmbData {
        nmea::sentences::RmbData {
            status: true,
            cross_track_error: Some(0.5),
            origin_waypoint_id: None,
            dest_waypoint_id: None,
            dest_latitude: Some(54.0),
            dest_longitude: Some(10.0),
            range_to_dest: Some(1.0),
            bearing_to_dest: Some(90.0),
            closing_velocity: Some(3.0),
            arrived: false,
        }
    }

    #[test]
    fn rmb_active_emits_all_fields() {
        let values = from_rmb(&rmb_active());
        assert!(!values.is_empty());

        let pos = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.nextPoint.position")
            .unwrap();
        assert_eq!(pos.value["latitude"].as_f64().unwrap(), 54.0);
        assert_eq!(pos.value["longitude"].as_f64().unwrap(), 10.0);

        let dist = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.nextPoint.distance")
            .unwrap();
        assert!((dist.value.as_f64().unwrap() - NM_TO_M).abs() < 1.0);

        let bearing = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.nextPoint.bearingTrue")
            .unwrap();
        assert!((bearing.value.as_f64().unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-6);

        let xte = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.crossTrackError")
            .unwrap();
        assert!((xte.value.as_f64().unwrap() - 0.5 * NM_TO_M).abs() < 1.0);
    }

    #[test]
    fn rmb_void_emits_nothing() {
        let rmb = nmea::sentences::RmbData {
            status: false,
            cross_track_error: Some(0.5),
            origin_waypoint_id: None,
            dest_waypoint_id: None,
            dest_latitude: Some(54.0),
            dest_longitude: Some(10.0),
            range_to_dest: Some(1.0),
            bearing_to_dest: Some(90.0),
            closing_velocity: Some(3.0),
            arrived: false,
        };
        assert!(from_rmb(&rmb).is_empty());
    }

    #[test]
    fn rmb_unit_conversions() {
        let values = from_rmb(&rmb_active());
        let dist = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.nextPoint.distance")
            .unwrap();
        assert_eq!(dist.value.as_f64().unwrap(), 1852.0);
        let bearing = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.nextPoint.bearingTrue")
            .unwrap();
        assert!((bearing.value.as_f64().unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn bwc_position_bearing_distance() {
        let bwc = nmea::sentences::BwcData {
            fix_time: None,
            latitude: Some(54.5),
            longitude: Some(10.2),
            true_bearing: Some(90.0),
            magnetic_bearing: Some(87.0),
            distance: Some(2.0),
            waypoint_id: None,
        };
        let values = from_bwc(&bwc);
        let pos = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.nextPoint.position")
            .unwrap();
        assert_eq!(pos.value["latitude"].as_f64().unwrap(), 54.5);

        let bt = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.nextPoint.bearingTrue")
            .unwrap();
        assert!((bt.value.as_f64().unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-5);

        let bm = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.nextPoint.bearingMagnetic")
            .unwrap();
        assert!((bm.value.as_f64().unwrap() - 87.0 * DEG_TO_RAD).abs() < 1e-5);

        let dist = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.nextPoint.distance")
            .unwrap();
        assert!((dist.value.as_f64().unwrap() - 2.0 * NM_TO_M).abs() < 1.0);
    }

    #[test]
    fn bod_bearing_true_and_magnetic() {
        let bod = nmea::sentences::BodData {
            bearing_true: Some(180.0),
            bearing_magnetic: Some(177.0),
            to_waypoint: None,
            from_waypoint: None,
        };
        let values = from_bod(&bod);
        let bt = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.bearingTrackTrue")
            .unwrap();
        assert!((bt.value.as_f64().unwrap() - std::f64::consts::PI).abs() < 1e-6);
        let bm = values
            .iter()
            .find(|p| p.path == "navigation.courseGreatCircle.bearingTrackMagnetic")
            .unwrap();
        assert!((bm.value.as_f64().unwrap() - 177.0 * DEG_TO_RAD).abs() < 1e-5);
    }

    #[test]
    fn xte_fault_emits_nothing() {
        let xte = nmea::sentences::XteData {
            cross_track_error: Some(0.1),
            status_general: false,
            status_cycle_lock: true,
        };
        assert!(from_xte(&xte).is_empty());
    }

    #[test]
    fn xte_nm_to_m() {
        let xte = nmea::sentences::XteData {
            cross_track_error: Some(0.5),
            status_general: true,
            status_cycle_lock: true,
        };
        let values = from_xte(&xte);
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].path,
            "navigation.courseGreatCircle.crossTrackError"
        );
        assert!((values[0].value.as_f64().unwrap() - 0.5 * NM_TO_M).abs() < 1.0);
    }
}
