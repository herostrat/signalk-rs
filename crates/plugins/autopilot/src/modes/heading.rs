/// Compass / heading-hold mode.
///
/// Reads `navigation.headingMagnetic` (primary) or `navigation.headingTrue`
/// (fallback) and steers to a target heading using the PID controller.
///
/// # Algorithm
///
/// ```text
/// error     = normalize(target − current_heading)
/// d_error   = −yaw_rate          (if navigation.rateOfTurn is available)
///           = normalize(error − prev_error) / dt  (finite-difference fallback)
/// rudder    = PID(error, d_error, dt)
/// ```
///
/// Using the actual yaw rate as the D-term is more stable than finite
/// differences: it is direct (no lag), not susceptible to compass quantisation
/// noise, and responds correctly even before heading error starts changing.
use crate::pd::{self, PidConfig, PidController};

/// Compute rudder command for heading hold.
///
/// - `pid`: mutable PID controller (maintains integral state across ticks)
/// - `yaw_rate`: `navigation.rateOfTurn` in rad/s if available, otherwise `None`
///
/// Returns `(rudder_rad, new_error_rad)`.
/// Store `new_error_rad` in `AutopilotState.last_error_rad` for the next tick.
pub fn compute(
    current_heading: f64,
    effective_target: f64,
    prev_error: f64,
    dt: f64,
    yaw_rate: Option<f64>,
    pid: &mut PidController,
    cfg: &PidConfig,
) -> (f64, f64) {
    let error = pd::normalize_angle(effective_target - current_heading);
    let d_error = match yaw_rate {
        // Yaw rate D-term: negative because positive rate means heading approaching
        // target (error shrinking), which should reduce corrective rudder.
        Some(rate) => -rate,
        // Finite-difference fallback: angle-safe delta / dt
        None => pd::normalize_angle(error - prev_error) / dt,
    };
    let rudder = pid.compute(error, d_error, dt, cfg);
    (rudder, error)
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

    #[test]
    fn on_course_returns_zero_rudder() {
        let mut pid = PidController::new(0.5);
        let (rudder, error) = compute(1.0, 1.0, 0.0, 1.0, None, &mut pid, &default_cfg());
        assert_eq!(rudder, 0.0);
        assert!(error.abs() < 1e-10);
    }

    #[test]
    fn positive_error_gives_positive_rudder() {
        let mut pid = PidController::new(0.5);
        let (rudder, _) = compute(0.0, 0.5, 0.0, 1.0, None, &mut pid, &default_cfg());
        assert!(rudder > 0.0);
    }

    #[test]
    fn negative_error_gives_negative_rudder() {
        let mut pid = PidController::new(0.5);
        let (rudder, _) = compute(0.5, 0.0, 0.0, 1.0, None, &mut pid, &default_cfg());
        assert!(rudder < 0.0);
    }

    #[test]
    fn error_wraps_at_pi_boundary() {
        let mut pid = PidController::new(0.5);
        let (_, error) = compute(
            -PI + 0.035,
            PI - 0.035,
            0.0,
            1.0,
            None,
            &mut pid,
            &default_cfg(),
        );
        assert!(error.abs() < 0.1, "error should be ~2°, got {error:.4} rad");
    }

    #[test]
    fn d_term_finite_diff_damps_converging_response() {
        let cfg = default_cfg();
        let mut pid1 = PidController::new(0.5);
        let mut pid2 = PidController::new(0.5);
        let (r_converging, _) = compute(0.0, 0.1, 0.2, 1.0, None, &mut pid1, &cfg);
        let (r_steady, _) = compute(0.0, 0.1, 0.1, 1.0, None, &mut pid2, &cfg);
        assert!(r_converging < r_steady);
    }

    #[test]
    fn d_term_yaw_rate_damps_converging_response() {
        let cfg = default_cfg();
        let mut pid1 = PidController::new(0.5);
        let mut pid2 = PidController::new(0.5);
        let (r_with_rate, _) = compute(0.0, 0.1, 0.0, 1.0, Some(0.05), &mut pid1, &cfg);
        let (r_no_rate, _) = compute(0.0, 0.1, 0.0, 1.0, Some(0.0), &mut pid2, &cfg);
        assert!(r_with_rate < r_no_rate);
    }

    #[test]
    fn yaw_rate_d_term_preferred_over_finite_diff() {
        let cfg = default_cfg();
        let mut pid1 = PidController::new(0.5);
        let mut pid2 = PidController::new(0.5);
        let (r_rate, _) = compute(0.0, 0.1, 0.0, 1.0, Some(0.0), &mut pid1, &cfg);
        let (r_diff, _) = compute(0.0, 0.1, 0.0, 1.0, None, &mut pid2, &cfg);
        assert!(r_diff > r_rate);
    }

    #[test]
    fn new_error_is_stored_for_next_tick() {
        let mut pid = PidController::new(0.5);
        let (_, new_error) = compute(0.0, 0.3, 0.0, 1.0, None, &mut pid, &default_cfg());
        assert!((new_error - 0.3).abs() < 1e-10);
    }
}
