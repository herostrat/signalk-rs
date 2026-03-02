/// Cascaded route-following mode (LOS guidance).
///
/// Two-stage control:
///
/// ```text
/// Outer loop (slow):
///   heading_correction = atan(XTE / lookahead_distance)
///
/// Inner loop (fast):
///   desired_heading = BTW + heading_correction
///   rudder = PID(desired_heading − current_heading)
/// ```
///
/// The outer loop converts cross-track error into a heading correction at a
/// rate proportional to the lookahead distance. The inner loop is the standard
/// heading PID from [`super::heading::compute`].
///
/// # Why cascaded?
///
/// A single-stage controller (XTE → rudder) lacks heading damping and tends
/// to oscillate around the track. The cascaded approach inherits the well-tuned
/// heading PID as the inner loop, giving good transient response and steady-state
/// tracking.
///
/// # Integration with Course API
///
/// Route mode reads two values from the SignalK store:
/// - `navigation.course.calcValues.bearingTrackTrue` — bearing to next waypoint
/// - `navigation.crossTrackError` — signed distance from track (m, + = starboard)
///
/// These are computed by the derived-data plugin from course state set via
/// the V2 Course API or bridged from NMEA instruments.
///
/// Waypoint advancement is automatic (CourseManager arrival detection) —
/// when the next waypoint changes, bearingTrackTrue and XTE update, and
/// the autopilot follows the new leg without intervention.
use crate::pd::{self, PidConfig, PidController};

/// Input parameters for route mode computation.
pub struct RouteInput {
    pub current_heading: f64,
    /// Bearing to waypoint (from SK `navigation.course.calcValues.bearingTrackTrue`)
    pub btw: f64,
    /// Cross-track error in metres (positive = starboard of track)
    pub xte_m: f64,
    /// LOS lookahead distance in metres
    pub lookahead_m: f64,
    /// Previous heading error (for D-term finite-difference fallback)
    pub prev_error: f64,
    /// Time since last tick in seconds
    pub dt: f64,
    /// `navigation.rateOfTurn` in rad/s if available
    pub yaw_rate: Option<f64>,
}

/// Compute rudder command for route following.
///
/// Returns `(rudder_rad, new_heading_error_rad)`.
pub fn compute(input: &RouteInput, pid: &mut PidController, cfg: &PidConfig) -> (f64, f64) {
    // Outer loop: XTE → heading correction via LOS guidance
    let lookahead = input.lookahead_m.max(1.0); // safety: never divide by tiny number
    let heading_correction = (input.xte_m / lookahead).atan();

    // Desired heading = bearing to waypoint + XTE correction
    let desired_heading = pd::normalize_angle(input.btw + heading_correction);

    // Inner loop: standard heading PID
    super::heading::compute(
        input.current_heading,
        desired_heading,
        input.prev_error,
        input.dt,
        input.yaw_rate,
        pid,
        cfg,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn default_cfg() -> PidConfig {
        PidConfig {
            gain_p: 1.0,
            gain_i: 0.05,
            gain_d: 0.3,
            dead_zone_rad: 0.01745,
            max_rudder_rad: std::f64::consts::FRAC_PI_6,
        }
    }

    fn make_input(heading: f64, btw: f64, xte_m: f64, lookahead_m: f64) -> RouteInput {
        RouteInput {
            current_heading: heading,
            btw,
            xte_m,
            lookahead_m,
            prev_error: 0.0,
            dt: 0.1,
            yaw_rate: None,
        }
    }

    #[test]
    fn on_track_and_on_heading_returns_zero() {
        let mut pid = PidController::new(0.5);
        let cfg = default_cfg();
        let (rudder, error) = compute(&make_input(0.0, 0.0, 0.0, 100.0), &mut pid, &cfg);
        assert_eq!(rudder, 0.0);
        assert!(error.abs() < 1e-10);
    }

    #[test]
    fn xte_starboard_corrects_to_port() {
        let mut pid = PidController::new(0.5);
        let cfg = default_cfg();
        let (rudder, _) = compute(&make_input(0.0, 0.0, 50.0, 100.0), &mut pid, &cfg);
        assert!(
            rudder > 0.0,
            "should steer toward track when starboard of it, got {rudder}"
        );
    }

    #[test]
    fn xte_port_corrects_to_starboard() {
        let mut pid = PidController::new(0.5);
        let cfg = default_cfg();
        let (rudder, _) = compute(&make_input(0.0, 0.0, -50.0, 100.0), &mut pid, &cfg);
        assert!(
            rudder < 0.0,
            "should steer toward track when port of it, got {rudder}"
        );
    }

    #[test]
    fn heading_correction_bounded_by_atan() {
        let mut pid = PidController::new(0.5);
        let cfg = default_cfg();
        let (_, error) = compute(&make_input(0.0, 0.0, 10000.0, 100.0), &mut pid, &cfg);
        assert!(
            error.abs() < PI / 2.0 + 0.1,
            "heading correction should be bounded by atan"
        );
    }

    #[test]
    fn lookahead_floor_prevents_division_issues() {
        let mut pid = PidController::new(0.5);
        let cfg = default_cfg();
        let (rudder, _) = compute(&make_input(0.0, 0.0, 10.0, 0.001), &mut pid, &cfg);
        assert!(rudder.is_finite());
    }
}
