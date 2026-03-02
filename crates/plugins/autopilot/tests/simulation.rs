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
//! # Disturbance model
//!
//! An optional constant `wind_bias` (rad/s) models the effect of crosswind or
//! current on heading — the vessel drifts away from course at a steady rate.
//! This is the key difference between PD (has steady-state offset) and PID
//! (eliminates it via the integral term).

use autopilot::modes::heading;
use autopilot::pd::{
    self, HeadingPlausibility, PidConfig, PidController, PlausibilityResult, RecoveryState,
    RudderFeedbackMonitor,
};
use std::f64::consts::PI;

// ─── Virtual vessel model ─────────────────────────────────────────────────────

struct VesselSim {
    heading: f64,
    /// Yaw-rate coefficient: rad/s per rad of commanded rudder
    turn_rate_coeff: f64,
    dt: f64,
    /// Constant heading disturbance: models crosswind / current drift (rad/s)
    wind_bias: f64,
}

impl VesselSim {
    fn new(initial_heading: f64) -> Self {
        VesselSim {
            heading: initial_heading,
            turn_rate_coeff: 0.3,
            dt: 0.1, // 10 Hz control loop
            wind_bias: 0.0,
        }
    }

    fn with_wind_bias(mut self, bias: f64) -> Self {
        self.wind_bias = bias;
        self
    }

    /// Apply rudder and advance one time step.
    fn step(&mut self, rudder_rad: f64) {
        self.heading = pd::normalize_angle(
            self.heading + (rudder_rad * self.turn_rate_coeff + self.wind_bias) * self.dt,
        );
    }
}

fn default_pid_cfg() -> PidConfig {
    PidConfig {
        gain_p: 1.0,
        gain_i: 0.05,
        gain_d: 0.3,
        dead_zone_rad: 0.01745,
        max_rudder_rad: std::f64::consts::FRAC_PI_6,
    }
}

/// Run the heading controller for `n_ticks` and return the trajectory.
///
/// Returns `Vec<(tick, heading, rudder, error)>`.
fn run_compass(
    vessel: &mut VesselSim,
    target: f64,
    n_ticks: usize,
    pid: &mut PidController,
    cfg: &PidConfig,
) -> Vec<(usize, f64, f64, f64)> {
    let mut prev_error = 0.0_f64;
    let mut trajectory = Vec::with_capacity(n_ticks);

    for tick in 0..n_ticks {
        let (rudder, new_error) = heading::compute(
            vessel.heading,
            target,
            prev_error,
            vessel.dt,
            None,
            pid,
            cfg,
        );
        trajectory.push((tick, vessel.heading, rudder, new_error));
        vessel.step(rudder);
        prev_error = new_error;
    }
    trajectory
}

// ─── Convergence tests ────────────────────────────────────────────────────────

/// Starting 30° off course, the autopilot must converge within 600 ticks (60s at 10 Hz).
#[test]
fn compass_converges_from_30deg_offset() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 6.0); // +30° initial error
    let target = 0.0;

    let traj = run_compass(&mut vessel, target, 600, &mut pid, &cfg);
    let final_heading = traj.last().map(|t| t.1).unwrap();
    let final_error = final_heading - target;

    assert!(
        final_error.abs() <= cfg.dead_zone_rad + 0.01,
        "should converge within dead zone after 600 ticks, final error: {final_error:.4} rad ({:.1}°)",
        final_error.to_degrees()
    );
}

/// Starting 90° off course, the autopilot must converge within 1200 ticks (120s at 10 Hz).
#[test]
fn compass_converges_from_90deg_offset() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 2.0); // +90° initial error
    let target = 0.0;

    let traj = run_compass(&mut vessel, target, 1200, &mut pid, &cfg);
    let final_heading = traj.last().map(|t| t.1).unwrap();
    let final_error = pd::normalize_angle(final_heading - target);

    assert!(
        final_error.abs() <= cfg.dead_zone_rad + 0.01,
        "should converge from 90° within 1200 ticks, final error: {final_error:.4} rad ({:.1}°)",
        final_error.to_degrees()
    );
}

/// After convergence, heading must not overshoot by more than 10° on the other side.
#[test]
fn compass_no_significant_overshoot() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 6.0); // +30°
    let target = 0.0;

    let traj = run_compass(&mut vessel, target, 900, &mut pid, &cfg);

    // Skip first 50 ticks (approach phase at 10 Hz = 5s)
    let max_overshoot = traj
        .iter()
        .skip(50)
        .map(|&(_, heading, _, _)| {
            let err = pd::normalize_angle(heading - target);
            if err < 0.0 { -err } else { 0.0 }
        })
        .fold(0.0_f64, f64::max);

    let max_overshoot_deg = max_overshoot.to_degrees();
    assert!(
        max_overshoot_deg < 10.0,
        "D-term should prevent overshoot > 10°, got {max_overshoot_deg:.1}°"
    );
}

// ─── Wrap-around test ────────────────────────────────────────────────────────

/// Crossing the ±180° boundary: a 2° turn should not become a 358° turn.
#[test]
fn compass_wraps_correctly_at_180_boundary() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let target = -(PI - 0.035); // -179°
    let mut vessel = VesselSim::new(PI - 0.035); // +179°

    let traj = run_compass(&mut vessel, target, 300, &mut pid, &cfg);
    let final_heading = traj.last().map(|t| t.1).unwrap();
    let final_error = pd::normalize_angle(final_heading - target);

    assert!(
        final_error.abs() <= cfg.dead_zone_rad + 0.05,
        "should cross ±180° cleanly, final error: {final_error:.4} rad ({:.1}°)",
        final_error.to_degrees()
    );

    // Verify it went the short way (headings near ±180°, not through 0°)
    let max_abs_heading = traj
        .iter()
        .map(|&(_, h, _, _)| h.abs())
        .fold(0.0_f64, f64::max);
    assert!(
        max_abs_heading > PI / 2.0,
        "boat should have stayed near ±180°, not gone through 0°"
    );
}

// ─── Tack test ───────────────────────────────────────────────────────────────

/// After a tack (wind mode), the controller must settle on the new heading
/// within 900 ticks (90s at 10 Hz).
#[test]
fn wind_tack_settles_within_900_ticks() {
    let cfg = default_pid_cfg();
    let starboard_tack = 0.7_f64;
    let port_tack = -0.7_f64;
    let mut vessel = VesselSim::new(starboard_tack);

    // Settle on starboard tack
    let mut pid = PidController::new(0.5);
    let _ = run_compass(&mut vessel, starboard_tack, 100, &mut pid, &cfg);

    // Tack to port: reset PID (as the real autopilot does)
    pid.reset();
    let mut prev_error = 0.0_f64;
    let n_ticks = 900;
    let mut trajectory = Vec::with_capacity(n_ticks);

    for tick in 0..n_ticks {
        let (rudder, new_error) = heading::compute(
            vessel.heading,
            port_tack,
            prev_error,
            vessel.dt,
            None,
            &mut pid,
            &cfg,
        );
        trajectory.push((tick, vessel.heading, rudder, new_error));
        vessel.step(rudder);
        prev_error = new_error;
    }

    let final_heading = trajectory.last().map(|t| t.1).unwrap();
    let final_error = pd::normalize_angle(final_heading - port_tack);

    assert!(
        final_error.abs() <= cfg.dead_zone_rad + 0.02,
        "should settle on port tack within 900 ticks, final error: {final_error:.4} rad ({:.1}°)",
        final_error.to_degrees()
    );
}

// ─── Stability test ──────────────────────────────────────────────────────────

/// With default gains, the controller must not hunt (oscillate) after convergence.
#[test]
fn compass_stable_no_hunting_after_convergence() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 6.0);
    let target = 0.0;

    let traj = run_compass(&mut vessel, target, 1200, &mut pid, &cfg);

    // Count rudder direction changes after convergence (skip first 300 ticks = 30s)
    let post_convergence: Vec<f64> = traj.iter().skip(300).map(|&(_, _, r, _)| r).collect();

    let sign_changes = post_convergence
        .windows(2)
        .filter(|w| w[0] != 0.0 && w[1] != 0.0 && w[0].signum() != w[1].signum())
        .count();

    assert!(
        sign_changes <= 3,
        "too many rudder direction changes after convergence ({sign_changes}) — gains may be too high"
    );
}

// ─── PID vs PD: disturbance rejection ────────────────────────────────────────

/// With a constant crosswind bias, PID (gain_i > 0) should have zero steady-state
/// error, while PD (gain_i = 0) has a permanent offset.
#[test]
fn pid_eliminates_steady_state_error_with_wind_bias() {
    let pid_cfg = default_pid_cfg();
    let pd_cfg = PidConfig {
        gain_i: 0.0,
        ..pid_cfg.clone()
    };

    let wind_bias = 0.01; // constant heading drift ~0.6 deg/s

    // PD controller
    let mut pd_pid = PidController::new(0.5);
    let mut pd_vessel = VesselSim::new(0.0).with_wind_bias(wind_bias);
    let pd_traj = run_compass(&mut pd_vessel, 0.0, 2000, &mut pd_pid, &pd_cfg);
    let pd_final_error = pd_traj.last().map(|t| t.3).unwrap().abs();

    // PID controller
    let mut pid_ctrl = PidController::new(0.5);
    let mut pid_vessel = VesselSim::new(0.0).with_wind_bias(wind_bias);
    let pid_traj = run_compass(&mut pid_vessel, 0.0, 2000, &mut pid_ctrl, &pid_cfg);
    let pid_final_error = pid_traj.last().map(|t| t.3).unwrap().abs();

    // PD should have a measurable steady-state offset
    assert!(
        pd_final_error > pid_cfg.dead_zone_rad,
        "PD should have steady-state error with wind bias, got {:.4} rad",
        pd_final_error
    );
    // PID should eliminate it (within dead zone)
    assert!(
        pid_final_error <= pid_cfg.dead_zone_rad + 0.01,
        "PID should eliminate steady-state error, got {:.4} rad ({:.1}°)",
        pid_final_error,
        pid_final_error.to_degrees()
    );
}

// ─── Recovery mode ──────────────────────────────────────────────────────────

/// Recovery mode should converge faster than normal PID for a sudden 40° deviation.
#[test]
fn recovery_converges_faster_than_normal() {
    let cfg = default_pid_cfg();
    let initial_error = 0.7; // ~40° — above 20° recovery threshold

    // Normal PID (no recovery)
    let mut pid_normal = PidController::new(0.5);
    let mut vessel_normal = VesselSim::new(initial_error);
    let traj_normal = run_compass(&mut vessel_normal, 0.0, 300, &mut pid_normal, &cfg);

    // PID with recovery (boosted gains)
    let mut pid_recovery = PidController::new(0.5);
    let mut recovery = RecoveryState::new();
    let mut vessel_recovery = VesselSim::new(initial_error);
    let boosted_cfg = PidConfig {
        gain_p: cfg.gain_p * 2.0,
        gain_i: 0.0, // recovery disables I
        gain_d: cfg.gain_d * 2.0,
        ..cfg.clone()
    };

    let mut prev_error_r = 0.0_f64;
    let mut traj_recovery = Vec::with_capacity(300);

    for tick in 0..300 {
        let error = pd::normalize_angle(0.0 - vessel_recovery.heading);
        let active = recovery.update(error, 0.35, 15);
        let active_cfg = if active { &boosted_cfg } else { &cfg };
        let (rudder, new_error) = heading::compute(
            vessel_recovery.heading,
            0.0,
            prev_error_r,
            vessel_recovery.dt,
            None,
            &mut pid_recovery,
            active_cfg,
        );
        traj_recovery.push((tick, vessel_recovery.heading, rudder, new_error));
        vessel_recovery.step(rudder);
        prev_error_r = new_error;
    }

    // Find tick at which error first drops below 5° for each
    let convergence_tick = |traj: &[(usize, f64, f64, f64)]| {
        traj.iter()
            .find(|&&(_, _, _, err)| err.abs() < 0.087) // ~5°
            .map(|&(t, _, _, _)| t)
    };

    let normal_tick = convergence_tick(&traj_normal);
    let recovery_tick = convergence_tick(&traj_recovery);

    match (normal_tick, recovery_tick) {
        (Some(n), Some(r)) => assert!(
            r <= n,
            "recovery should converge no later than normal: recovery={r}, normal={n}"
        ),
        (None, Some(_)) => {} // recovery converges, normal doesn't — great
        (Some(_), None) => panic!("recovery should converge at least as fast as normal"),
        (None, None) => panic!("neither mode converged within 300 ticks"),
    }
}

/// Recovery mode must not cause excessive overshoot (safety: H13).
#[test]
fn recovery_no_excessive_overshoot() {
    let cfg = default_pid_cfg();
    let boosted_cfg = PidConfig {
        gain_p: cfg.gain_p * 2.0,
        gain_i: 0.0,
        gain_d: cfg.gain_d * 2.0,
        ..cfg.clone()
    };

    let mut pid = PidController::new(0.5);
    let mut recovery = RecoveryState::new();
    let mut vessel = VesselSim::new(0.7); // 40° offset
    let mut prev_error = 0.0_f64;

    let mut max_overshoot = 0.0_f64;
    for _ in 0..600 {
        let error = pd::normalize_angle(0.0 - vessel.heading);
        let active = recovery.update(error, 0.35, 15);
        let active_cfg = if active { &boosted_cfg } else { &cfg };
        let (rudder, new_error) = heading::compute(
            vessel.heading,
            0.0,
            prev_error,
            vessel.dt,
            None,
            &mut pid,
            active_cfg,
        );
        vessel.step(rudder);
        prev_error = new_error;

        // Track overshoot (heading crossing zero to the wrong side)
        let heading_err = pd::normalize_angle(vessel.heading);
        if heading_err < 0.0 {
            max_overshoot = max_overshoot.max(-heading_err);
        }
    }

    let max_overshoot_deg = max_overshoot.to_degrees();
    assert!(
        max_overshoot_deg < 15.0,
        "recovery should not overshoot > 15°, got {max_overshoot_deg:.1}°"
    );
}

// ─── Gust response ──────────────────────────────────────────────────────────

/// The gust feedforward should reduce peak heading deviation during a wind gust.
#[test]
fn gust_feedforward_reduces_peak_deviation() {
    use autopilot::filter::RateDetector;

    let cfg = default_pid_cfg();
    let gust_gain = -0.02_f64;
    let gust_threshold = 3.0_f64;

    // Simulate a steady-state boat, then a gust at tick 200
    // The gust causes a heading disturbance proportional to wind speed increase

    // Without gust feedforward
    let mut pid_no_gust = PidController::new(0.5);
    let mut vessel_no_gust = VesselSim::new(0.0);
    let mut prev_error_ng = 0.0_f64;
    let mut max_deviation_no_gust = 0.0_f64;

    for tick in 0..500 {
        // Gust: sudden heading disturbance at tick 200 (simulating wind push)
        let wind_disturbance = if (200..220).contains(&tick) {
            0.02
        } else {
            0.0
        };
        vessel_no_gust.wind_bias = wind_disturbance;

        let (rudder, new_error) = heading::compute(
            vessel_no_gust.heading,
            0.0,
            prev_error_ng,
            vessel_no_gust.dt,
            None,
            &mut pid_no_gust,
            &cfg,
        );
        vessel_no_gust.step(rudder);
        prev_error_ng = new_error;
        max_deviation_no_gust = max_deviation_no_gust.max(vessel_no_gust.heading.abs());
    }

    // With gust feedforward
    let mut pid_gust = PidController::new(0.5);
    let mut gust_detector = RateDetector::new(0.5);
    let mut vessel_gust = VesselSim::new(0.0);
    let mut prev_error_g = 0.0_f64;
    let mut max_deviation_gust = 0.0_f64;

    for tick in 0..500 {
        let wind_speed = if tick >= 200 { 15.0 } else { 10.0 }; // 5 m/s gust
        let wind_disturbance = if (200..220).contains(&tick) {
            0.02
        } else {
            0.0
        };
        vessel_gust.wind_bias = wind_disturbance;

        let rate = gust_detector.update(wind_speed, vessel_gust.dt);
        let gust_ff = if rate.abs() > gust_threshold {
            gust_gain * rate
        } else {
            0.0
        };

        let (raw_rudder, new_error) = heading::compute(
            vessel_gust.heading,
            0.0,
            prev_error_g,
            vessel_gust.dt,
            None,
            &mut pid_gust,
            &cfg,
        );
        let rudder = (raw_rudder + gust_ff).clamp(-cfg.max_rudder_rad, cfg.max_rudder_rad);
        vessel_gust.step(rudder);
        prev_error_g = new_error;
        max_deviation_gust = max_deviation_gust.max(vessel_gust.heading.abs());
    }

    // Gust feedforward should reduce peak deviation
    assert!(
        max_deviation_gust <= max_deviation_no_gust,
        "gust ff should reduce deviation: with={:.4} without={:.4}",
        max_deviation_gust,
        max_deviation_no_gust
    );
}

// ─── Rudder feedback monitoring ─────────────────────────────────────────────

/// Simulates a working rudder that tracks the commanded angle with a small delay.
/// The feedback monitor should never fire an alarm in normal operation.
#[test]
fn rudder_feedback_no_alarm_when_tracking() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 6.0); // 30° offset
    let mut feedback = RudderFeedbackMonitor::new();
    let mut prev_error = 0.0_f64;
    let mut actual_rudder = 0.0_f64;
    let threshold = 0.087; // 5°
    let timeout = 30;

    for _ in 0..600 {
        let (commanded, new_error) = heading::compute(
            vessel.heading,
            0.0,
            prev_error,
            vessel.dt,
            None,
            &mut pid,
            &cfg,
        );
        // Simulate actuator: actual rudder chases commanded with first-order lag
        actual_rudder += (commanded - actual_rudder) * 0.3; // 30% per tick ≈ fast hydraulic
        let result = feedback.update(commanded, Some(actual_rudder), threshold, timeout);
        assert_ne!(
            result,
            Some(true),
            "alarm should not fire with working actuator"
        );
        vessel.step(commanded);
        prev_error = new_error;
    }
    assert!(!feedback.is_alarm_active());
}

/// Simulates a stuck rudder (actual = 0 regardless of command).
/// The feedback monitor should fire an alarm after timeout_ticks.
#[test]
fn rudder_feedback_detects_stuck_rudder() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 6.0); // 30° offset
    let mut feedback = RudderFeedbackMonitor::new();
    let mut prev_error = 0.0_f64;
    let threshold = 0.087;
    let timeout = 30;
    let mut alarm_tick = None;

    for tick in 0..200 {
        let (commanded, new_error) = heading::compute(
            vessel.heading,
            0.0,
            prev_error,
            vessel.dt,
            None,
            &mut pid,
            &cfg,
        );
        // Stuck rudder: actual is always 0
        let result = feedback.update(commanded, Some(0.0), threshold, timeout);
        if result == Some(true) && alarm_tick.is_none() {
            alarm_tick = Some(tick);
        }
        // Vessel doesn't actually turn (rudder stuck at 0)
        vessel.step(0.0);
        prev_error = new_error;
    }

    assert!(
        feedback.is_alarm_active(),
        "alarm should be active with stuck rudder"
    );
    let tick = alarm_tick.expect("alarm should have fired");
    // Should fire around tick 30 (timeout_ticks), give or take a few
    // (first few ticks the commanded rudder is still building up)
    assert!(
        tick <= 50,
        "alarm should fire within ~50 ticks, fired at {tick}"
    );
}

/// When the rudder recovers (un-sticks), the alarm should clear.
#[test]
fn rudder_feedback_clears_on_recovery() {
    let mut feedback = RudderFeedbackMonitor::new();
    let threshold = 0.087;
    let timeout = 5;

    // Trigger alarm: 5 ticks of mismatch
    for _ in 0..5 {
        feedback.update(0.3, Some(0.0), threshold, timeout);
    }
    assert!(feedback.is_alarm_active());

    // Rudder catches up
    let result = feedback.update(0.1, Some(0.1), threshold, timeout);
    assert_eq!(result, Some(false), "alarm should clear on recovery");
    assert!(!feedback.is_alarm_active());
}

/// Without a rudder sensor (actual = None), no alarm should ever fire.
#[test]
fn rudder_feedback_dormant_without_sensor() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 6.0);
    let mut feedback = RudderFeedbackMonitor::new();
    let mut prev_error = 0.0_f64;

    for _ in 0..200 {
        let (commanded, new_error) = heading::compute(
            vessel.heading,
            0.0,
            prev_error,
            vessel.dt,
            None,
            &mut pid,
            &cfg,
        );
        let result = feedback.update(commanded, None, 0.087, 30);
        assert!(result.is_none(), "should be dormant without sensor");
        vessel.step(commanded);
        prev_error = new_error;
    }
    assert!(!feedback.is_alarm_active());
}

// ─── Heading plausibility ─────────────────────────────────────────────────

/// Persistent 180° heading spikes (EMI / compass failure) must cause
/// SensorFailure after max_consecutive_glitches (3).
#[test]
fn heading_glitch_causes_disengage() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 6.0);
    let mut plausibility = HeadingPlausibility::new(3);
    let mut prev_error = 0.0_f64;
    let max_rate = 1.5; // rad/s
    let dt = 0.1;
    let mut disengaged = false;

    for tick in 0..200 {
        // Inject 180° spike starting at tick 100
        let raw_heading = if (100..111).contains(&tick) {
            pd::normalize_angle(vessel.heading + PI) // flip 180°
        } else {
            vessel.heading
        };

        let heading = match plausibility.check(raw_heading, max_rate, dt) {
            PlausibilityResult::Ok(v) => v,
            PlausibilityResult::Glitch(prev) => prev,
            PlausibilityResult::SensorFailure => {
                disengaged = true;
                break;
            }
        };

        let (rudder, new_error) =
            heading::compute(heading, 0.0, prev_error, dt, None, &mut pid, &cfg);
        vessel.step(rudder);
        prev_error = new_error;
    }

    assert!(
        disengaged,
        "should disengage after 3 consecutive heading glitches"
    );
}

/// A single-tick heading spike should be discarded (use prev heading),
/// and the autopilot should converge normally.
#[test]
fn single_heading_glitch_does_not_disengage() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 6.0); // 30° offset
    let mut plausibility = HeadingPlausibility::new(3);
    let mut prev_error = 0.0_f64;
    let max_rate = 1.5;
    let dt = 0.1;
    let mut glitch_count = 0;

    for tick in 0..600 {
        // Single-tick 180° spike at tick 100
        let raw_heading = if tick == 100 {
            pd::normalize_angle(vessel.heading + PI)
        } else {
            vessel.heading
        };

        let heading = match plausibility.check(raw_heading, max_rate, dt) {
            PlausibilityResult::Ok(v) => v,
            PlausibilityResult::Glitch(prev) => {
                glitch_count += 1;
                prev
            }
            PlausibilityResult::SensorFailure => {
                panic!("should NOT disengage on a single glitch");
            }
        };

        let (rudder, new_error) =
            heading::compute(heading, 0.0, prev_error, dt, None, &mut pid, &cfg);
        vessel.step(rudder);
        prev_error = new_error;
    }

    assert_eq!(glitch_count, 1, "exactly one glitch should be detected");

    // Should still converge
    let final_error = pd::normalize_angle(vessel.heading).abs();
    assert!(
        final_error <= cfg.dead_zone_rad + 0.01,
        "should converge despite single glitch, final error: {:.4} rad",
        final_error
    );
}

/// Implausible yaw rate readings (e.g. 50 rad/s) should be rejected,
/// falling back to finite-difference D-term. The controller stays stable.
#[test]
fn yaw_rate_glitch_falls_back_to_finite_diff() {
    let cfg = default_pid_cfg();
    let mut pid = PidController::new(0.5);
    let mut vessel = VesselSim::new(PI / 6.0);
    let mut prev_error = 0.0_f64;
    let max_yaw_rate = 0.8;

    for tick in 0..600 {
        // Inject implausible yaw rate between tick 100-120
        let raw_yaw = if (100..120).contains(&tick) {
            Some(50.0) // 50 rad/s — physically impossible
        } else {
            None // use finite-diff fallback
        };

        let yaw_rate = pd::validate_yaw_rate(raw_yaw, max_yaw_rate);

        let (rudder, new_error) = heading::compute(
            vessel.heading,
            0.0,
            prev_error,
            vessel.dt,
            yaw_rate,
            &mut pid,
            &cfg,
        );
        vessel.step(rudder);
        prev_error = new_error;
    }

    let final_error = pd::normalize_angle(vessel.heading).abs();
    assert!(
        final_error <= cfg.dead_zone_rad + 0.01,
        "should converge despite yaw rate glitches, final error: {:.4} rad",
        final_error
    );
}

/// Stale sensor data (high heading_age_secs) should reduce the D-term via
/// sensor_quality scaling, producing measurably different rudder output.
#[test]
fn stale_sensor_reduces_d_term_effect() {
    let cfg = default_pid_cfg();
    let half_life = 0.5;

    // Fresh sensor: quality ≈ 1.0, full D-term
    let fresh_quality = pd::sensor_quality(0.0, half_life);
    let fresh_cfg = PidConfig {
        gain_d: cfg.gain_d * fresh_quality,
        ..cfg.clone()
    };

    // Stale sensor: age = 1.0s → quality ≈ 0.25, reduced D-term
    let stale_quality = pd::sensor_quality(1.0, half_life);
    let stale_cfg = PidConfig {
        gain_d: cfg.gain_d * stale_quality,
        ..cfg.clone()
    };

    // Verify quality values are as expected
    assert!(
        (fresh_quality - 1.0).abs() < 1e-10,
        "fresh quality should be 1.0"
    );
    assert!(
        stale_quality < 0.3,
        "stale quality should be < 0.3, got {stale_quality}"
    );

    // Run both with same initial conditions and measure rudder at first tick.
    // Use a small heading error so the output is NOT clamped to max_rudder,
    // making the D-term contribution visible.
    let mut pid_fresh = PidController::new(0.5);
    let mut pid_stale = PidController::new(0.5);
    let small_error = 0.1; // ~6° — well below max_rudder

    // Use yaw_rate to exercise the D-term
    let (rudder_fresh, _) = heading::compute(
        small_error,
        0.0,
        0.0,
        0.1,
        Some(0.1),
        &mut pid_fresh,
        &fresh_cfg,
    );
    let (rudder_stale, _) = heading::compute(
        small_error,
        0.0,
        0.0,
        0.1,
        Some(0.1),
        &mut pid_stale,
        &stale_cfg,
    );

    // With reduced D-gain, the D-term contribution is smaller → different rudder
    // (the yaw rate opposes error, so less D-term means less damping → bigger rudder)
    assert!(
        (rudder_fresh - rudder_stale).abs() > 0.001,
        "stale sensor should change rudder output: fresh={rudder_fresh:.4}, stale={rudder_stale:.4}"
    );
}
