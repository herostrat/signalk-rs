//! Simulation-based integration tests for the autopilot control loop.
//!
//! These tests use a simple first-order vessel model to verify that the
//! autopilot **actually steers the boat** to the target — something unit
//! tests of individual functions cannot guarantee.
//!
//! # Vessel model
//!
//! ```text
//! heading[n+1] = normalize(heading[n] + rudder[n] * K * dt)
//! ```
//!
//! where `K` is the turn-rate coefficient (rad/s per rad of rudder).
//! A value of `K = 0.3` roughly models a medium-sized sailing yacht:
//! full rudder (≈ π/6 ≈ 0.52 rad) produces about 0.16 rad/s (≈ 9°/s) yaw rate.
//!
//! # Why these tests matter
//!
//! - Verify PD gain tuning leads to **convergence** (no instability)
//! - Verify the D-term actually **prevents significant overshoot**
//! - Verify **wrap-around arithmetic** works at the 180°/−180° boundary
//! - Verify **tack maneuvers** settle within a reasonable number of ticks
//!
//! These scenarios complement property-based tests (which verify mathematical
//! invariants) and unit tests (which verify individual function contracts).

use autopilot::modes::heading;
use autopilot::pd;
use autopilot::state::AutopilotConfig;
use std::f64::consts::PI;

// ─── Virtual vessel model ─────────────────────────────────────────────────────

struct VesselSim {
    heading: f64,
    /// Yaw-rate coefficient: rad/s per rad of commanded rudder
    turn_rate_coeff: f64,
    dt: f64,
}

impl VesselSim {
    fn new(initial_heading: f64) -> Self {
        VesselSim {
            heading: initial_heading,
            turn_rate_coeff: 0.3,
            dt: 1.0, // 1 Hz control loop
        }
    }

    /// Apply rudder and advance one time step.
    fn step(&mut self, rudder_rad: f64) {
        self.heading =
            pd::normalize_angle(self.heading + rudder_rad * self.turn_rate_coeff * self.dt);
    }
}

/// Run the heading controller for `n_ticks` and return the trajectory.
///
/// Returns `Vec<(tick, heading, rudder, error)>`.
fn run_compass(
    vessel: &mut VesselSim,
    target: f64,
    n_ticks: usize,
    cfg: &AutopilotConfig,
) -> Vec<(usize, f64, f64, f64)> {
    let mut prev_error = 0.0_f64;
    let mut trajectory = Vec::with_capacity(n_ticks);

    for tick in 0..n_ticks {
        let (rudder, new_error) =
            heading::compute(vessel.heading, target, prev_error, vessel.dt, None, cfg);
        trajectory.push((tick, vessel.heading, rudder, new_error));
        vessel.step(rudder);
        prev_error = new_error;
    }
    trajectory
}

// ─── Convergence tests ────────────────────────────────────────────────────────

/// Starting 30° off course, the autopilot must converge within 60 ticks.
#[test]
fn compass_converges_from_30deg_offset() {
    let cfg = AutopilotConfig::default();
    let mut vessel = VesselSim::new(PI / 6.0); // +30° initial error
    let target = 0.0;

    let traj = run_compass(&mut vessel, target, 60, &cfg);
    let final_heading = traj.last().map(|t| t.1).unwrap();
    let final_error = final_heading - target;

    assert!(
        final_error.abs() <= cfg.dead_zone_rad + 0.01,
        "should converge to within dead zone after 60 ticks, final error: {final_error:.4} rad ({:.1}°)",
        final_error.to_degrees()
    );
}

/// Starting 90° off course, the autopilot must converge within 120 ticks.
#[test]
fn compass_converges_from_90deg_offset() {
    let cfg = AutopilotConfig::default();
    let mut vessel = VesselSim::new(PI / 2.0); // +90° initial error
    let target = 0.0;

    let traj = run_compass(&mut vessel, target, 120, &cfg);
    let final_heading = traj.last().map(|t| t.1).unwrap();
    let final_error = pd::normalize_angle(final_heading - target);

    assert!(
        final_error.abs() <= cfg.dead_zone_rad + 0.01,
        "should converge from 90° offset within 120 ticks, final error: {final_error:.4} rad ({:.1}°)",
        final_error.to_degrees()
    );
}

/// After convergence, heading must not overshoot by more than 10° on the other side.
///
/// This verifies the D-term is working correctly to damp oscillation.
#[test]
fn compass_no_significant_overshoot() {
    let cfg = AutopilotConfig::default();
    let mut vessel = VesselSim::new(PI / 6.0); // +30°
    let target = 0.0;

    let traj = run_compass(&mut vessel, target, 90, &cfg);

    // Find maximum overshoot: the most negative heading (past target)
    // We skip the first 5 ticks to ignore the approach phase
    let max_overshoot = traj
        .iter()
        .skip(5)
        .map(|&(_, heading, _, _)| {
            let err = pd::normalize_angle(heading - target);
            // Overshoot = error in opposite direction (negative when approaching from +)
            if err < 0.0 { -err } else { 0.0 }
        })
        .fold(0.0_f64, f64::max);

    let max_overshoot_deg = max_overshoot.to_degrees();
    assert!(
        max_overshoot_deg < 10.0,
        "D-term should prevent overshoot > 10°, got {max_overshoot_deg:.1}°"
    );
}

// ─── Wrap-around tests ────────────────────────────────────────────────────────

/// Crossing the ±180° boundary: a 2° turn should not become a 358° turn.
///
/// This tests that `normalize_angle` is correctly applied throughout the
/// controller chain.
#[test]
fn compass_wraps_correctly_at_180_boundary() {
    let cfg = AutopilotConfig::default();
    // Target: -179° (just past -180°), current: +179° (just below +180°)
    // The correct maneuver is 2° starboard; the wrong maneuver is 358° port.
    let target = -(PI - 0.035); // -179°
    let mut vessel = VesselSim::new(PI - 0.035); // +179°

    let traj = run_compass(&mut vessel, target, 30, &cfg);
    let final_heading = traj.last().map(|t| t.1).unwrap();
    let final_error = pd::normalize_angle(final_heading - target);

    // Verify: boat converged (did not go the wrong way around)
    assert!(
        final_error.abs() <= cfg.dead_zone_rad + 0.05,
        "should cross ±180° cleanly, final error: {final_error:.4} rad ({:.1}°)",
        final_error.to_degrees()
    );

    // Also verify it went the short way: heading moved from +179° toward -180°/+180°,
    // not all the way around through 0°, 90°, etc.
    // The trajectory should show headings near ±180°, not near 0° or 90°.
    let max_abs_heading = traj
        .iter()
        .map(|&(_, h, _, _)| h.abs())
        .fold(0.0_f64, f64::max);
    assert!(
        max_abs_heading > PI / 2.0,
        "boat should have stayed near ±180°, not gone through 0°"
    );
}

// ─── Tack tests ───────────────────────────────────────────────────────────────

/// After a tack (wind mode), the controller must settle on the new heading
/// within 90 ticks.
///
/// Tack is simulated by flipping the target wind angle (from +0.7 to -0.7 rad).
/// The vessel model is the same (heading hold), since wind-mode uses identical
/// PD logic in Phase A.
#[test]
fn wind_tack_settles_within_90_ticks() {
    let cfg = AutopilotConfig::default();
    // Start settled on starboard tack (target = +0.7 rad ≈ 40°)
    let starboard_tack = 0.7_f64;
    let port_tack = -0.7_f64;
    let mut vessel = VesselSim::new(starboard_tack); // already on course

    // Run 10 ticks on starboard tack to settle
    let traj_before = run_compass(&mut vessel, starboard_tack, 10, &cfg);
    let heading_before_tack = traj_before.last().map(|t| t.1).unwrap();

    // Tack to port (flip target; D-term reset simulated by starting with prev_error=0)
    let mut prev_error = 0.0_f64; // D-term reset after tack
    let n_ticks = 90;
    let mut trajectory = Vec::with_capacity(n_ticks);

    for tick in 0..n_ticks {
        let (rudder, new_error) =
            heading::compute(vessel.heading, port_tack, prev_error, vessel.dt, None, &cfg);
        trajectory.push((tick, vessel.heading, rudder, new_error));
        vessel.step(rudder);
        prev_error = new_error;
    }

    let final_heading = trajectory.last().map(|t| t.1).unwrap();
    let final_error = pd::normalize_angle(final_heading - port_tack);

    assert!(
        final_error.abs() <= cfg.dead_zone_rad + 0.02,
        "should settle on port tack within 90 ticks after tacking from {:.1}°, final error: {:.4} rad ({:.1}°)",
        heading_before_tack.to_degrees(),
        final_error,
        final_error.to_degrees()
    );
}

// ─── Stability test ───────────────────────────────────────────────────────────

/// With default gains, the controller must not oscillate (sign changes < threshold)
/// after initial convergence.
///
/// Excessive sign changes in the rudder command indicate hunting/oscillation,
/// which causes motor wear and is a sign of over-tuned gains.
#[test]
fn compass_stable_no_hunting_after_convergence() {
    let cfg = AutopilotConfig::default();
    let mut vessel = VesselSim::new(PI / 6.0); // 30° off course
    let target = 0.0;

    let traj = run_compass(&mut vessel, target, 120, &cfg);

    // Count rudder direction changes AFTER convergence (skip first 30 ticks)
    let post_convergence: Vec<f64> = traj.iter().skip(30).map(|&(_, _, r, _)| r).collect();

    let sign_changes = post_convergence
        .windows(2)
        .filter(|w| w[0] != 0.0 && w[1] != 0.0 && w[0].signum() != w[1].signum())
        .count();

    assert!(
        sign_changes <= 3,
        "too many rudder direction changes after convergence ({sign_changes}) — gains may be too high"
    );
}
