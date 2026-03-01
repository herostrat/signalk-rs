/// PD (proportional-derivative) controller core algorithms.
///
/// Implements the pure mathematical building blocks:
/// - [`normalize_angle`]: wrap to \[-π, π\]
/// - [`compute_rudder`]: PD formula with dead zone and clamp
///
/// These functions are deliberately free of state — all mutable state lives
/// in `AutopilotState` and is managed by the control loop in `lib.rs`.
use std::f64::consts::PI;

/// Normalize an angle to the range `[-π, π]`.
///
/// Essential for heading and wind angle arithmetic where wrapping occurs
/// (e.g. 355° → 5° change is +10°, not -350°).
pub fn normalize_angle(a: f64) -> f64 {
    let mut x = a;
    while x > PI {
        x -= 2.0 * PI;
    }
    while x < -PI {
        x += 2.0 * PI;
    }
    x
}

/// Compute rudder angle using a PD controller.
///
/// `error_rad`       — signed heading/wind angle error (target − current), normalized to [-π, π]
/// `d_error_rad`     — rate of change of error: (error − prev_error) / dt_secs
/// `gain_p`          — proportional gain (larger = more aggressive correction)
/// `gain_d`          — derivative gain (larger = more damping, less overshoot)
/// `dead_zone_rad`   — errors within ±dead_zone produce zero output (prevents hunting)
/// `max_rudder_rad`  — maximum rudder deflection (output is clamped to ±max_rudder)
///
/// Returns: commanded rudder angle in radians (+ve = starboard, −ve = port).
pub fn compute_rudder(
    error_rad: f64,
    d_error_rad: f64,
    gain_p: f64,
    gain_d: f64,
    dead_zone_rad: f64,
    max_rudder_rad: f64,
) -> f64 {
    // Safety guard: NaN/Inf inputs produce zero output rather than propagating
    // into the actuator command. This covers corrupted sensor values (H7).
    if !error_rad.is_finite() || !d_error_rad.is_finite() {
        return 0.0;
    }
    if error_rad.abs() < dead_zone_rad {
        return 0.0;
    }
    (gain_p * error_rad + gain_d * d_error_rad).clamp(-max_rudder_rad, max_rudder_rad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::f64::consts::PI;

    #[test]
    fn normalize_positive_wrap() {
        let a = normalize_angle(PI + 0.1);
        assert!(a < 0.0, "should wrap to negative side");
        assert!((a - (-PI + 0.1)).abs() < 1e-10);
    }

    #[test]
    fn normalize_negative_wrap() {
        let a = normalize_angle(-PI - 0.1);
        assert!(a > 0.0, "should wrap to positive side");
        assert!((a - (PI - 0.1)).abs() < 1e-10);
    }

    #[test]
    fn normalize_passthrough() {
        assert!((normalize_angle(0.5) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn dead_zone_suppresses_small_error() {
        assert_eq!(compute_rudder(0.005, 0.0, 1.0, 0.5, 0.01, 0.5), 0.0);
    }

    #[test]
    fn proportional_response() {
        let r = compute_rudder(0.1, 0.0, 2.0, 0.0, 0.01, 0.5);
        assert!((r - 0.2).abs() < 1e-10);
    }

    #[test]
    fn derivative_damps_response() {
        // Same error but positive d_error (error growing) → larger rudder
        let r_growing = compute_rudder(0.1, 0.02, 2.0, 1.0, 0.01, 1.0);
        // Same error but d_error=0 (steady)
        let r_steady = compute_rudder(0.1, 0.0, 2.0, 1.0, 0.01, 1.0);
        assert!(r_growing > r_steady);
    }

    #[test]
    fn derivative_reduces_response_when_converging() {
        // Error is positive but shrinking (d_error < 0) → D-term reduces output
        let r_converging = compute_rudder(0.1, -0.05, 2.0, 1.0, 0.01, 1.0);
        let r_steady = compute_rudder(0.1, 0.0, 2.0, 1.0, 0.01, 1.0);
        assert!(r_converging < r_steady);
    }

    #[test]
    fn clamps_to_max_rudder() {
        let r = compute_rudder(1.0, 0.0, 2.0, 0.0, 0.01, 0.5);
        assert!((r - 0.5).abs() < 1e-10);
    }

    #[test]
    fn negative_error_gives_negative_rudder() {
        let r = compute_rudder(-0.2, 0.0, 1.0, 0.0, 0.01, 0.5);
        assert!(r < 0.0);
        assert!((r - (-0.2)).abs() < 1e-10);
    }

    #[test]
    fn nan_error_returns_zero_rudder() {
        // Safety H7: NaN/Inf inputs must not propagate to actuator
        assert_eq!(compute_rudder(f64::NAN, 0.0, 1.0, 0.3, 0.01, 0.5), 0.0);
        assert_eq!(compute_rudder(0.1, f64::NAN, 1.0, 0.3, 0.01, 0.5), 0.0);
        assert_eq!(compute_rudder(f64::INFINITY, 0.0, 1.0, 0.3, 0.01, 0.5), 0.0);
    }

    // ── Property-based tests ───────────────────────────────────────────────────

    proptest! {
        /// normalize_angle always returns a value in [-π, π].
        #[test]
        fn normalize_always_in_range(a in -1e6_f64..1e6_f64) {
            let r = normalize_angle(a);
            prop_assert!((-PI..=PI).contains(&r), "normalize_angle({a}) = {r}");
        }

        /// normalize_angle is idempotent: applying it twice is the same as once.
        #[test]
        fn normalize_idempotent(a in -1e6_f64..1e6_f64) {
            let once = normalize_angle(a);
            let twice = normalize_angle(once);
            prop_assert!((once - twice).abs() < 1e-12);
        }

        /// compute_rudder output is always within [-max_rudder, +max_rudder].
        #[test]
        fn rudder_always_clamped(
            error in -PI..PI,
            d_error in -10.0_f64..10.0_f64,
            gain_p in 0.0_f64..10.0_f64,
            gain_d in 0.0_f64..10.0_f64,
            dead_zone in 0.0_f64..0.1_f64,
            max_rudder in 0.01_f64..PI,
        ) {
            let r = compute_rudder(error, d_error, gain_p, gain_d, dead_zone, max_rudder);
            prop_assert!(
                r >= -max_rudder && r <= max_rudder,
                "rudder {r} out of [{}, {}]", -max_rudder, max_rudder
            );
        }

        /// If |error| < dead_zone, rudder is always zero (no hunting in dead zone).
        #[test]
        fn dead_zone_always_zero(
            error in -0.05_f64..0.05_f64,
            d_error in -1.0_f64..1.0_f64,
        ) {
            let dead_zone = 0.1; // larger than the error range
            let r = compute_rudder(error, d_error, 1.0, 0.5, dead_zone, PI);
            prop_assert_eq!(r, 0.0);
        }
    }
}
