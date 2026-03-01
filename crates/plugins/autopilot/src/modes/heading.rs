/// Compass / heading-hold mode.
///
/// Reads `navigation.headingMagnetic` (primary) or `navigation.headingTrue`
/// (fallback) and steers to a target heading using the PD controller.
///
/// # Algorithm
///
/// ```text
/// error     = normalize(target − current_heading)
/// d_error   = −yaw_rate          (if navigation.rateOfTurn is available)
///           = normalize(error − prev_error) / dt  (finite-difference fallback)
/// rudder    = clamp(P * error + D * d_error, −max, +max)
/// ```
///
/// Using the actual yaw rate as the D-term is more stable than finite
/// differences: it is direct (no lag), not susceptible to compass quantisation
/// noise, and responds correctly even before heading error starts changing.
///
/// Sign convention: a positive yaw rate (heading increasing = turning starboard)
/// produces a negative d_error, which reduces the rudder command and prevents
/// overshoot when the boat is converging on the target.
use crate::{pd, state::AutopilotConfig};

/// Compute rudder command for heading hold.
///
/// - `yaw_rate`: `navigation.rateOfTurn` in rad/s if available, otherwise `None`.
///   When `Some`, it is used as the D-term directly (more stable than finite diff).
///
/// Returns `(rudder_rad, new_error_rad)`.
/// Store `new_error_rad` in `AutopilotState.last_error_rad` for the next tick.
pub fn compute(
    current_heading: f64,
    effective_target: f64,
    prev_error: f64,
    dt: f64,
    yaw_rate: Option<f64>,
    cfg: &AutopilotConfig,
) -> (f64, f64) {
    let error = pd::normalize_angle(effective_target - current_heading);
    let d_error = match yaw_rate {
        // Yaw rate D-term: negative because positive rate means heading approaching
        // target (error shrinking), which should reduce corrective rudder.
        Some(rate) => -rate,
        // Finite-difference fallback: angle-safe delta / dt
        None => pd::normalize_angle(error - prev_error) / dt,
    };
    let rudder = pd::compute_rudder(
        error,
        d_error,
        cfg.gain_p,
        cfg.gain_d,
        cfg.dead_zone_rad,
        cfg.max_rudder_rad,
    );
    (rudder, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AutopilotConfig;
    use std::f64::consts::PI;

    fn cfg() -> AutopilotConfig {
        AutopilotConfig::default()
    }

    #[test]
    fn on_course_returns_zero_rudder() {
        let (rudder, error) = compute(1.0, 1.0, 0.0, 1.0, None, &cfg());
        assert_eq!(rudder, 0.0);
        assert!(error.abs() < 1e-10);
    }

    #[test]
    fn positive_error_gives_positive_rudder() {
        let (rudder, _) = compute(0.0, 0.5, 0.0, 1.0, None, &cfg());
        assert!(rudder > 0.0);
    }

    #[test]
    fn negative_error_gives_negative_rudder() {
        let (rudder, _) = compute(0.5, 0.0, 0.0, 1.0, None, &cfg());
        assert!(rudder < 0.0);
    }

    #[test]
    fn error_wraps_at_pi_boundary() {
        // Target: just below 180°, current: just above -180°
        // Error should be small positive (~2°), not large negative (~-358°)
        let (_, error) = compute(-PI + 0.035, PI - 0.035, 0.0, 1.0, None, &cfg());
        assert!(error.abs() < 0.1, "error should be ~2°, got {error:.4} rad");
    }

    #[test]
    fn d_term_finite_diff_damps_converging_response() {
        let c = cfg();
        // prev_error > current error → error shrinking → d_error negative → smaller rudder
        let (r_converging, _) = compute(0.0, 0.1, 0.2, 1.0, None, &c);
        let (r_steady, _) = compute(0.0, 0.1, 0.1, 1.0, None, &c); // no change in error
        assert!(
            r_converging < r_steady,
            "converging should give smaller rudder than steady"
        );
    }

    #[test]
    fn d_term_yaw_rate_damps_converging_response() {
        let c = cfg();
        // Positive yaw rate (turning toward target) → d_error negative → smaller rudder
        let (r_with_rate, _) = compute(0.0, 0.1, 0.0, 1.0, Some(0.05), &c);
        let (r_no_rate, _) = compute(0.0, 0.1, 0.0, 1.0, Some(0.0), &c);
        assert!(
            r_with_rate < r_no_rate,
            "positive yaw rate should reduce rudder command"
        );
    }

    #[test]
    fn yaw_rate_d_term_preferred_over_finite_diff() {
        // With yaw_rate=Some(0.0), d_error=0 (no rate feedback); with None, finite diff kicks in
        let c = cfg();
        // Both produce rudder, but the magnitudes will differ when prev_error != error
        let (r_rate, _) = compute(0.0, 0.1, 0.0, 1.0, Some(0.0), &c);
        let (r_diff, _) = compute(0.0, 0.1, 0.0, 1.0, None, &c);
        // With prev_error=0 and dt=1, finite diff d_error = 0.1/1 = 0.1 → gain_d*0.1 extra
        // yaw_rate=0: d_error=0 → P-only
        assert!(
            r_diff > r_rate,
            "finite diff with growing error should give larger rudder than zero yaw rate"
        );
    }

    #[test]
    fn new_error_is_stored_for_next_tick() {
        let (_, new_error) = compute(0.0, 0.3, 0.0, 1.0, None, &cfg());
        // error = normalize(0.3 - 0.0) = 0.3
        assert!((new_error - 0.3).abs() < 1e-10);
    }
}
