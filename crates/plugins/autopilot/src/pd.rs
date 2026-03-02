/// PID controller, gain scheduling, and rudder rate limiting.
///
/// Core control algorithms for the autopilot:
/// - [`normalize_angle`]: wrap to \[-π, π\]
/// - [`PidController`]: stateful PID with anti-windup
/// - [`PidConfig`]: controller tuning parameters
/// - [`scale_gains`]: speed-dependent Nomoto gain scheduling
/// - [`rate_limit`]: actuator slew-rate limiting
use std::f64::consts::PI;

// ─── Angle normalization ─────────────────────────────────────────────────────

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

// ─── PID configuration ──────────────────────────────────────────────────────

/// Controller tuning parameters. Passed to [`PidController::compute`].
///
/// All angles in radians.
#[derive(Debug, Clone)]
pub struct PidConfig {
    pub gain_p: f64,
    pub gain_i: f64,
    pub gain_d: f64,
    pub dead_zone_rad: f64,
    pub max_rudder_rad: f64,
}

// ─── PID controller ─────────────────────────────────────────────────────────

/// PID controller with anti-windup.
///
/// Maintains the integral accumulator across ticks. Create one per control
/// loop lifetime; call [`reset`](PidController::reset) on mode change or
/// disengage.
///
/// # Anti-windup scheme
///
/// The integral is only updated when the output is not saturated (clamped to
/// `max_rudder_rad`). Exception: if the error sign opposes the integral sign,
/// integration continues — this allows the integral to unwind when the
/// disturbance reverses.
///
/// Inside the dead zone, the integral decays by 5% per tick to prevent drift.
pub struct PidController {
    integral: f64,
    integral_limit: f64,
}

impl PidController {
    /// Create a new PID controller.
    ///
    /// `integral_limit` caps the absolute integral accumulation (radians).
    pub fn new(integral_limit: f64) -> Self {
        PidController {
            integral: 0.0,
            integral_limit,
        }
    }

    /// Reset the integral accumulator (call on mode change, disengage, tack).
    pub fn reset(&mut self) {
        self.integral = 0.0;
    }

    /// Current integral value (for diagnostics/testing).
    pub fn integral(&self) -> f64 {
        self.integral
    }

    /// Compute rudder command.
    ///
    /// - `error_rad`: signed heading/wind error (target − current), normalized to [-π, π]
    /// - `d_error_rad`: rate of change of error (−yaw_rate or finite difference)
    /// - `dt`: time since last tick in seconds
    /// - `cfg`: controller gains and limits
    ///
    /// Returns: commanded rudder angle in radians (+ve = starboard, −ve = port).
    pub fn compute(&mut self, error_rad: f64, d_error_rad: f64, dt: f64, cfg: &PidConfig) -> f64 {
        // Safety guard H7: NaN/Inf inputs produce zero output
        if !error_rad.is_finite() || !d_error_rad.is_finite() || !dt.is_finite() || dt <= 0.0 {
            return 0.0;
        }

        // Dead zone: decay integral to prevent drift, output zero
        if error_rad.abs() < cfg.dead_zone_rad {
            self.integral *= 0.95;
            return 0.0;
        }

        // PD portion (without integral)
        let pd_output = cfg.gain_p * error_rad + cfg.gain_d * d_error_rad;

        // Anti-windup: only integrate when not saturated, or when error
        // opposes integral (allowing unwind)
        let prospective = pd_output + cfg.gain_i * self.integral;
        if prospective.abs() < cfg.max_rudder_rad || error_rad.signum() != self.integral.signum() {
            self.integral += error_rad * dt;
            self.integral = self
                .integral
                .clamp(-self.integral_limit, self.integral_limit);
        }

        let output = cfg.gain_p * error_rad + cfg.gain_i * self.integral + cfg.gain_d * d_error_rad;
        output.clamp(-cfg.max_rudder_rad, cfg.max_rudder_rad)
    }
}

// ─── Gain scheduling ─────────────────────────────────────────────────────────

/// Scale PID gains based on vessel speed (Nomoto principle).
///
/// P and I scale proportionally with speed (more responsive at higher speed).
/// D scales inversely (less damping needed at speed — rudder is more effective).
///
/// Returns unmodified config if `speed_nominal_mps <= 0` or `speed_mps` is
/// invalid (disabled / no data).
pub fn scale_gains(cfg: &PidConfig, speed_mps: f64, speed_nominal_mps: f64) -> PidConfig {
    if speed_nominal_mps <= 0.0 || !speed_mps.is_finite() || speed_mps <= 0.0 {
        return cfg.clone();
    }
    let ratio = (speed_mps / speed_nominal_mps).clamp(0.3, 3.0);
    PidConfig {
        gain_p: cfg.gain_p * ratio,
        gain_i: cfg.gain_i * ratio,
        gain_d: cfg.gain_d / ratio,
        ..*cfg
    }
}

// ─── Recovery mode ──────────────────────────────────────────────────────────

/// Temporary aggressive-gain mode for large sudden deviations (wave, gust).
///
/// When the heading error exceeds `threshold_rad`, the recovery mode activates
/// for at most `max_ticks` ticks. While active, gains are multiplied by
/// `gain_factor` and the integral term is disabled (prevents windup during
/// the large transient).
///
/// Recovery deactivates when the error drops below 30% of the threshold or
/// when `max_ticks` expires.
pub struct RecoveryState {
    active: bool,
    ticks_remaining: u32,
}

impl Default for RecoveryState {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryState {
    pub fn new() -> Self {
        RecoveryState {
            active: false,
            ticks_remaining: 0,
        }
    }

    /// Check whether recovery should be (de)activated based on current error.
    ///
    /// Call once per tick. Returns `true` if recovery is currently active.
    pub fn update(&mut self, error_rad: f64, threshold_rad: f64, max_ticks: u32) -> bool {
        if threshold_rad <= 0.0 {
            return false;
        }
        if !self.active && error_rad.abs() > threshold_rad {
            self.active = true;
            self.ticks_remaining = max_ticks;
        }
        if self.active {
            self.ticks_remaining = self.ticks_remaining.saturating_sub(1);
            if self.ticks_remaining == 0 || error_rad.abs() < threshold_rad * 0.3 {
                self.active = false;
            }
        }
        self.active
    }

    /// Modify PID gains for recovery. Disables I-term, boosts P and D.
    pub fn apply(&self, cfg: &PidConfig, gain_factor: f64) -> PidConfig {
        if !self.active {
            return cfg.clone();
        }
        PidConfig {
            gain_p: cfg.gain_p * gain_factor,
            gain_i: 0.0,
            gain_d: cfg.gain_d * gain_factor,
            ..*cfg
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.ticks_remaining = 0;
    }
}

// ─── Rudder feedback monitor ─────────────────────────────────────────────

/// Monitors commanded vs actual rudder angle for feedback failure detection.
///
/// Tracks consecutive ticks where the mismatch exceeds a threshold. After
/// `timeout_ticks` consecutive mismatches, the alarm fires. The alarm clears
/// when the mismatch drops below the threshold.
///
/// If no rudder feedback sensor is available (`update` is never called with
/// `Some(actual)`), the monitor stays dormant — no false alarms.
pub struct RudderFeedbackMonitor {
    mismatch_ticks: u32,
    alarm_active: bool,
}

impl Default for RudderFeedbackMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl RudderFeedbackMonitor {
    pub fn new() -> Self {
        RudderFeedbackMonitor {
            mismatch_ticks: 0,
            alarm_active: false,
        }
    }

    /// Check rudder feedback for this tick.
    ///
    /// - `commanded`: rudder angle the autopilot is requesting
    /// - `actual`: rudder angle from hardware sensor (`None` if no sensor)
    /// - `threshold_rad`: maximum acceptable mismatch (0 = disabled)
    /// - `timeout_ticks`: consecutive mismatches before alarm
    ///
    /// Returns state change: `Some(true)` = alarm just fired,
    /// `Some(false)` = alarm just cleared, `None` = no change.
    pub fn update(
        &mut self,
        commanded: f64,
        actual: Option<f64>,
        threshold_rad: f64,
        timeout_ticks: u32,
    ) -> Option<bool> {
        if threshold_rad <= 0.0 {
            return None;
        }
        let actual = actual?;

        let mismatch = (commanded - actual).abs();
        if mismatch > threshold_rad {
            self.mismatch_ticks += 1;
            if self.mismatch_ticks >= timeout_ticks && !self.alarm_active {
                self.alarm_active = true;
                return Some(true); // alarm fired
            }
        } else {
            self.mismatch_ticks = 0;
            if self.alarm_active {
                self.alarm_active = false;
                return Some(false); // alarm cleared
            }
        }
        None
    }

    pub fn is_alarm_active(&self) -> bool {
        self.alarm_active
    }

    pub fn reset(&mut self) {
        self.mismatch_ticks = 0;
        self.alarm_active = false;
    }
}

// ─── Heading plausibility ───────────────────────────────────────────────────

/// Result of a heading plausibility check.
#[derive(Debug, Clone, PartialEq)]
pub enum PlausibilityResult {
    /// Value is plausible — use it.
    Ok(f64),
    /// Single spike detected — use the previous heading (fallback).
    Glitch(f64),
    /// N consecutive glitches — sensor failure, must disengage.
    SensorFailure,
}

/// Detects implausible heading jumps (EMI spike, compass glitch).
///
/// A jump larger than `max_rate × dt` is classified as a glitch. The previous
/// heading is preserved (not updated) during a glitch so that a return to the
/// real heading is correctly recognized as plausible.
///
/// After `max_consecutive_glitches` consecutive glitches → [`PlausibilityResult::SensorFailure`].
pub struct HeadingPlausibility {
    prev_value: Option<f64>,
    glitch_count: u32,
    max_consecutive_glitches: u32,
}

impl HeadingPlausibility {
    pub fn new(max_consecutive_glitches: u32) -> Self {
        HeadingPlausibility {
            prev_value: None,
            glitch_count: 0,
            max_consecutive_glitches,
        }
    }

    /// Check whether a new heading value is plausible.
    ///
    /// - `value`: new heading in radians
    /// - `max_rate_rad_per_sec`: maximum plausible heading change rate
    /// - `dt`: time since last check in seconds
    pub fn check(&mut self, value: f64, max_rate_rad_per_sec: f64, dt: f64) -> PlausibilityResult {
        let prev = match self.prev_value {
            Some(p) => p,
            None => {
                // First sample — always accept
                self.prev_value = Some(value);
                self.glitch_count = 0;
                return PlausibilityResult::Ok(value);
            }
        };

        let delta = normalize_angle(value - prev).abs();
        let max_delta = max_rate_rad_per_sec * dt.max(0.0);

        if delta <= max_delta {
            // Plausible — update prev, reset glitch counter
            self.prev_value = Some(value);
            self.glitch_count = 0;
            PlausibilityResult::Ok(value)
        } else {
            // Glitch — do NOT update prev_value (so return to real heading is recognized)
            self.glitch_count += 1;
            if self.glitch_count >= self.max_consecutive_glitches {
                PlausibilityResult::SensorFailure
            } else {
                PlausibilityResult::Glitch(prev)
            }
        }
    }

    pub fn reset(&mut self) {
        self.prev_value = None;
        self.glitch_count = 0;
    }
}

// ─── Sensor quality ─────────────────────────────────────────────────────────

/// Exponential decay for D-term weighting based on sensor staleness.
///
/// `quality = 2^(-age / half_life)`, clamped to `[0, 1]`.
///
/// - `age_secs = 0` → 1.0 (fresh)
/// - `age_secs = half_life` → 0.5
/// - `age_secs = 4 × half_life` → 0.0625
///
/// Returns 0.0 for non-finite or negative inputs. Returns 1.0 if
/// `half_life_secs <= 0` (disabled).
pub fn sensor_quality(age_secs: f64, half_life_secs: f64) -> f64 {
    if !age_secs.is_finite() || !half_life_secs.is_finite() {
        return 0.0;
    }
    if half_life_secs <= 0.0 {
        return 1.0;
    }
    if age_secs < 0.0 {
        return 1.0;
    }
    (2.0_f64).powf(-age_secs / half_life_secs).clamp(0.0, 1.0)
}

/// Validate a yaw rate reading against a physical plausibility bound.
///
/// Returns `None` if the rate exceeds `max_rate` or is non-finite,
/// signaling the caller to fall back to finite-difference D-term.
pub fn validate_yaw_rate(rate: Option<f64>, max_rate_rad_per_sec: f64) -> Option<f64> {
    let r = rate?;
    if !r.is_finite() || r.abs() > max_rate_rad_per_sec {
        None
    } else {
        Some(r)
    }
}

// ─── Rate limiting ───────────────────────────────────────────────────────────

/// Limit rudder movement to a maximum rate.
///
/// `max_rate_rad_per_sec <= 0.0` disables limiting (returns `target` directly).
pub fn rate_limit(current: f64, target: f64, max_rate_rad_per_sec: f64, dt: f64) -> f64 {
    if max_rate_rad_per_sec <= 0.0 || !dt.is_finite() || dt <= 0.0 {
        return target;
    }
    let max_delta = max_rate_rad_per_sec * dt;
    current + (target - current).clamp(-max_delta, max_delta)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn default_cfg() -> PidConfig {
        PidConfig {
            gain_p: 1.0,
            gain_i: 0.05,
            gain_d: 0.3,
            dead_zone_rad: 0.01745,
            max_rudder_rad: std::f64::consts::FRAC_PI_6,
        }
    }

    // ── normalize_angle ─────────────────────────────────────────────────────

    #[test]
    fn normalize_positive_wrap() {
        let a = normalize_angle(PI + 0.1);
        assert!(a < 0.0);
        assert!((a - (-PI + 0.1)).abs() < 1e-10);
    }

    #[test]
    fn normalize_negative_wrap() {
        let a = normalize_angle(-PI - 0.1);
        assert!(a > 0.0);
        assert!((a - (PI - 0.1)).abs() < 1e-10);
    }

    #[test]
    fn normalize_passthrough() {
        assert!((normalize_angle(0.5) - 0.5).abs() < 1e-10);
    }

    // ── PidController: basic PD behavior ────────────────────────────────────

    #[test]
    fn dead_zone_suppresses_small_error() {
        let mut pid = PidController::new(0.5);
        let cfg = default_cfg();
        assert_eq!(pid.compute(0.005, 0.0, 1.0, &cfg), 0.0);
    }

    #[test]
    fn proportional_response() {
        let mut pid = PidController::new(0.5);
        let cfg = PidConfig {
            gain_p: 2.0,
            gain_i: 0.0,
            gain_d: 0.0,
            dead_zone_rad: 0.01,
            max_rudder_rad: 0.5,
        };
        let r = pid.compute(0.1, 0.0, 1.0, &cfg);
        assert!((r - 0.2).abs() < 1e-10);
    }

    #[test]
    fn derivative_damps_response() {
        let mut pid1 = PidController::new(0.5);
        let mut pid2 = PidController::new(0.5);
        let cfg = PidConfig {
            gain_p: 2.0,
            gain_i: 0.0,
            gain_d: 1.0,
            dead_zone_rad: 0.01,
            max_rudder_rad: 1.0,
        };
        let r_growing = pid1.compute(0.1, 0.02, 1.0, &cfg);
        let r_steady = pid2.compute(0.1, 0.0, 1.0, &cfg);
        assert!(r_growing > r_steady);
    }

    #[test]
    fn clamps_to_max_rudder() {
        let mut pid = PidController::new(0.5);
        let cfg = PidConfig {
            gain_p: 2.0,
            gain_i: 0.0,
            gain_d: 0.0,
            dead_zone_rad: 0.01,
            max_rudder_rad: 0.5,
        };
        let r = pid.compute(1.0, 0.0, 1.0, &cfg);
        assert!((r - 0.5).abs() < 1e-10);
    }

    #[test]
    fn negative_error_gives_negative_rudder() {
        let mut pid = PidController::new(0.5);
        let cfg = PidConfig {
            gain_p: 1.0,
            gain_i: 0.0,
            gain_d: 0.0,
            dead_zone_rad: 0.01,
            max_rudder_rad: 0.5,
        };
        let r = pid.compute(-0.2, 0.0, 1.0, &cfg);
        assert!(r < 0.0);
        assert!((r - (-0.2)).abs() < 1e-10);
    }

    #[test]
    fn nan_error_returns_zero_rudder() {
        let mut pid = PidController::new(0.5);
        let cfg = default_cfg();
        assert_eq!(pid.compute(f64::NAN, 0.0, 1.0, &cfg), 0.0);
        assert_eq!(pid.compute(0.1, f64::NAN, 1.0, &cfg), 0.0);
        assert_eq!(pid.compute(f64::INFINITY, 0.0, 1.0, &cfg), 0.0);
        assert_eq!(pid.compute(0.1, 0.0, f64::NAN, &cfg), 0.0);
    }

    // ── PidController: integral behavior ────────────────────────────────────

    #[test]
    fn integral_accumulates_over_ticks() {
        let mut pid = PidController::new(1.0);
        let cfg = PidConfig {
            gain_p: 0.0,
            gain_i: 1.0,
            gain_d: 0.0,
            dead_zone_rad: 0.01,
            max_rudder_rad: 1.0,
        };
        pid.compute(0.1, 0.0, 1.0, &cfg);
        pid.compute(0.1, 0.0, 1.0, &cfg);
        pid.compute(0.1, 0.0, 1.0, &cfg);
        assert!((pid.integral() - 0.3).abs() < 1e-10);
    }

    #[test]
    fn integral_eliminates_steady_state_error() {
        let mut pid = PidController::new(1.0);
        let cfg = PidConfig {
            gain_p: 0.5,
            gain_i: 0.2,
            gain_d: 0.0,
            dead_zone_rad: 0.001,
            max_rudder_rad: 1.0,
        };
        // Simulate constant small error over many ticks
        let error = 0.05; // 3° — too small for P alone to overcome disturbance
        let mut total_output = 0.0;
        for _ in 0..100 {
            total_output = pid.compute(error, 0.0, 1.0, &cfg);
        }
        // After 100 ticks, integral has built up → output much larger than P-only
        let p_only = cfg.gain_p * error;
        assert!(
            total_output > p_only * 2.0,
            "integral should boost output beyond P-only ({total_output} vs {p_only})"
        );
    }

    #[test]
    fn anti_windup_prevents_integral_growth_at_saturation() {
        let mut pid = PidController::new(1.0);
        let cfg = PidConfig {
            gain_p: 2.0,
            gain_i: 1.0,
            gain_d: 0.0,
            dead_zone_rad: 0.01,
            max_rudder_rad: 0.5,
        };
        // Large error → P alone saturates output → integral should NOT grow
        for _ in 0..50 {
            pid.compute(1.0, 0.0, 1.0, &cfg);
        }
        // Integral should be much less than 50 * 1.0 because anti-windup kicks in
        assert!(
            pid.integral().abs() < 5.0,
            "anti-windup should limit integral, got {}",
            pid.integral()
        );
    }

    #[test]
    fn integral_decays_in_dead_zone() {
        let mut pid = PidController::new(1.0);
        let cfg = default_cfg();
        // Build up some integral
        for _ in 0..20 {
            pid.compute(0.1, 0.0, 1.0, &cfg);
        }
        let integral_before = pid.integral();
        assert!(integral_before > 0.0);
        // Now enter dead zone
        for _ in 0..20 {
            pid.compute(0.005, 0.0, 1.0, &cfg);
        }
        assert!(
            pid.integral() < integral_before * 0.5,
            "integral should decay in dead zone"
        );
    }

    #[test]
    fn reset_clears_integral() {
        let mut pid = PidController::new(1.0);
        let cfg = default_cfg();
        pid.compute(0.1, 0.0, 1.0, &cfg);
        pid.compute(0.1, 0.0, 1.0, &cfg);
        assert!(pid.integral() > 0.0);
        pid.reset();
        assert_eq!(pid.integral(), 0.0);
    }

    #[test]
    fn integral_clamped_to_limit() {
        let mut pid = PidController::new(0.2); // small limit
        let cfg = PidConfig {
            gain_p: 0.0,
            gain_i: 1.0,
            gain_d: 0.0,
            dead_zone_rad: 0.001,
            max_rudder_rad: 10.0, // large so no saturation
        };
        for _ in 0..100 {
            pid.compute(0.1, 0.0, 1.0, &cfg);
        }
        assert!(
            pid.integral().abs() <= 0.2 + 1e-10,
            "integral should be clamped to limit, got {}",
            pid.integral()
        );
    }

    // ── scale_gains ─────────────────────────────────────────────────────────

    #[test]
    fn scale_gains_at_nominal_unchanged() {
        let cfg = default_cfg();
        let scaled = scale_gains(&cfg, 2.5, 2.5);
        assert!((scaled.gain_p - cfg.gain_p).abs() < 1e-10);
        assert!((scaled.gain_i - cfg.gain_i).abs() < 1e-10);
        assert!((scaled.gain_d - cfg.gain_d).abs() < 1e-10);
    }

    #[test]
    fn scale_gains_double_speed() {
        let cfg = default_cfg();
        let scaled = scale_gains(&cfg, 5.0, 2.5);
        assert!((scaled.gain_p - cfg.gain_p * 2.0).abs() < 1e-10);
        assert!((scaled.gain_d - cfg.gain_d / 2.0).abs() < 1e-10);
    }

    #[test]
    fn scale_gains_clamped_at_extremes() {
        let cfg = default_cfg();
        // Very low speed → ratio clamped to 0.3
        let low = scale_gains(&cfg, 0.1, 2.5);
        assert!((low.gain_p - cfg.gain_p * 0.3).abs() < 1e-10);
        // Very high speed → ratio clamped to 3.0
        let high = scale_gains(&cfg, 100.0, 2.5);
        assert!((high.gain_p - cfg.gain_p * 3.0).abs() < 1e-10);
    }

    #[test]
    fn scale_gains_disabled_when_nominal_zero() {
        let cfg = default_cfg();
        let scaled = scale_gains(&cfg, 5.0, 0.0);
        assert!((scaled.gain_p - cfg.gain_p).abs() < 1e-10);
    }

    #[test]
    fn scale_gains_nan_speed_returns_unchanged() {
        let cfg = default_cfg();
        let scaled = scale_gains(&cfg, f64::NAN, 2.5);
        assert!((scaled.gain_p - cfg.gain_p).abs() < 1e-10);
    }

    // ── rate_limit ──────────────────────────────────────────────────────────

    #[test]
    fn rate_limit_disabled_when_zero() {
        assert!((rate_limit(0.0, 1.0, 0.0, 1.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn rate_limit_constrains_positive_change() {
        let r = rate_limit(0.0, 1.0, 0.09, 1.0);
        assert!((r - 0.09).abs() < 1e-10);
    }

    #[test]
    fn rate_limit_constrains_negative_change() {
        let r = rate_limit(0.0, -1.0, 0.09, 1.0);
        assert!((r - (-0.09)).abs() < 1e-10);
    }

    #[test]
    fn rate_limit_allows_small_change() {
        let r = rate_limit(0.0, 0.05, 0.09, 1.0);
        assert!((r - 0.05).abs() < 1e-10);
    }

    #[test]
    fn rate_limit_scales_with_dt() {
        // At dt=0.1 (10 Hz), max delta = 0.09 * 0.1 = 0.009
        let r = rate_limit(0.0, 1.0, 0.09, 0.1);
        assert!((r - 0.009).abs() < 1e-10);
    }

    // ── RecoveryState ─────────────────────────────────────────────────────

    #[test]
    fn recovery_activates_on_large_error() {
        let mut r = RecoveryState::new();
        assert!(!r.is_active());
        let active = r.update(0.5, 0.35, 15);
        assert!(active);
        assert!(r.is_active());
    }

    #[test]
    fn recovery_does_not_activate_below_threshold() {
        let mut r = RecoveryState::new();
        let active = r.update(0.2, 0.35, 15);
        assert!(!active);
    }

    #[test]
    fn recovery_deactivates_on_low_error() {
        let mut r = RecoveryState::new();
        r.update(0.5, 0.35, 15); // activate
        // Error drops to 10% of threshold → below 30% → deactivate
        let active = r.update(0.03, 0.35, 15);
        assert!(!active);
    }

    #[test]
    fn recovery_deactivates_on_timeout() {
        let mut r = RecoveryState::new();
        // Activation call consumes tick 1 (ticks_remaining: 3→2)
        assert!(r.update(0.5, 0.35, 3));
        // Tick 2 (ticks_remaining: 2→1)
        assert!(r.update(0.4, 0.35, 3));
        // Tick 3 (ticks_remaining: 1→0 → deactivate)
        assert!(!r.update(0.4, 0.35, 3));
    }

    #[test]
    fn recovery_apply_boosts_gains() {
        let mut r = RecoveryState::new();
        let cfg = default_cfg();
        r.update(0.5, 0.35, 15);
        let boosted = r.apply(&cfg, 2.0);
        assert!((boosted.gain_p - cfg.gain_p * 2.0).abs() < 1e-10);
        assert!((boosted.gain_d - cfg.gain_d * 2.0).abs() < 1e-10);
        assert_eq!(boosted.gain_i, 0.0); // I-term disabled
    }

    #[test]
    fn recovery_apply_passthrough_when_inactive() {
        let r = RecoveryState::new();
        let cfg = default_cfg();
        let result = r.apply(&cfg, 2.0);
        assert!((result.gain_p - cfg.gain_p).abs() < 1e-10);
    }

    #[test]
    fn recovery_disabled_when_threshold_zero() {
        let mut r = RecoveryState::new();
        assert!(!r.update(1.0, 0.0, 15));
    }

    #[test]
    fn recovery_reset_clears_state() {
        let mut r = RecoveryState::new();
        r.update(0.5, 0.35, 15);
        assert!(r.is_active());
        r.reset();
        assert!(!r.is_active());
    }

    // ── RudderFeedbackMonitor ──────────────────────────────────────────────

    #[test]
    fn feedback_no_sensor_returns_none() {
        let mut m = RudderFeedbackMonitor::new();
        assert!(m.update(0.1, None, 0.087, 30).is_none());
        assert!(!m.is_alarm_active());
    }

    #[test]
    fn feedback_disabled_when_threshold_zero() {
        let mut m = RudderFeedbackMonitor::new();
        assert!(m.update(0.1, Some(0.0), 0.0, 30).is_none());
    }

    #[test]
    fn feedback_no_alarm_when_matching() {
        let mut m = RudderFeedbackMonitor::new();
        for _ in 0..50 {
            assert!(m.update(0.1, Some(0.1), 0.087, 30).is_none());
        }
        assert!(!m.is_alarm_active());
    }

    #[test]
    fn feedback_alarm_fires_after_timeout() {
        let mut m = RudderFeedbackMonitor::new();
        // 29 mismatches — no alarm yet
        for _ in 0..29 {
            assert!(m.update(0.3, Some(0.0), 0.087, 30).is_none());
        }
        assert!(!m.is_alarm_active());
        // 30th mismatch triggers alarm
        assert_eq!(m.update(0.3, Some(0.0), 0.087, 30), Some(true));
        assert!(m.is_alarm_active());
    }

    #[test]
    fn feedback_alarm_does_not_re_fire() {
        let mut m = RudderFeedbackMonitor::new();
        for _ in 0..30 {
            m.update(0.3, Some(0.0), 0.087, 30);
        }
        assert!(m.is_alarm_active());
        // Further mismatches return None (alarm already active)
        assert!(m.update(0.3, Some(0.0), 0.087, 30).is_none());
    }

    #[test]
    fn feedback_alarm_clears_on_match() {
        let mut m = RudderFeedbackMonitor::new();
        for _ in 0..30 {
            m.update(0.3, Some(0.0), 0.087, 30);
        }
        assert!(m.is_alarm_active());
        // Rudder catches up → alarm clears
        assert_eq!(m.update(0.1, Some(0.1), 0.087, 30), Some(false));
        assert!(!m.is_alarm_active());
    }

    #[test]
    fn feedback_mismatch_counter_resets_on_match() {
        let mut m = RudderFeedbackMonitor::new();
        // Build up 20 mismatch ticks
        for _ in 0..20 {
            m.update(0.3, Some(0.0), 0.087, 30);
        }
        // One matching tick resets the counter
        m.update(0.1, Some(0.1), 0.087, 30);
        // Need full 30 again to trigger
        for i in 0..30 {
            let result = m.update(0.3, Some(0.0), 0.087, 30);
            if i < 29 {
                assert!(result.is_none());
            } else {
                assert_eq!(result, Some(true));
            }
        }
    }

    #[test]
    fn feedback_reset_clears_alarm() {
        let mut m = RudderFeedbackMonitor::new();
        for _ in 0..30 {
            m.update(0.3, Some(0.0), 0.087, 30);
        }
        assert!(m.is_alarm_active());
        m.reset();
        assert!(!m.is_alarm_active());
    }

    // ── HeadingPlausibility ──────────────────────────────────────────────

    #[test]
    fn plausibility_first_sample_always_ok() {
        let mut h = HeadingPlausibility::new(3);
        match h.check(1.5, 1.5, 0.1) {
            PlausibilityResult::Ok(v) => assert!((v - 1.5).abs() < 1e-10),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn plausibility_normal_change_passes() {
        let mut h = HeadingPlausibility::new(3);
        h.check(0.0, 1.5, 0.1); // init
        // 0.1 rad change in 0.1s = 1.0 rad/s, under max 1.5
        match h.check(0.1, 1.5, 0.1) {
            PlausibilityResult::Ok(v) => assert!((v - 0.1).abs() < 1e-10),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn plausibility_single_glitch_returns_prev() {
        let mut h = HeadingPlausibility::new(3);
        h.check(0.0, 1.5, 0.1); // init
        // 2.0 rad change in 0.1s = 20 rad/s, way over max 1.5
        match h.check(2.0, 1.5, 0.1) {
            PlausibilityResult::Glitch(prev) => assert!((prev - 0.0).abs() < 1e-10),
            other => panic!("expected Glitch, got {other:?}"),
        }
    }

    #[test]
    fn plausibility_consecutive_glitches_cause_failure() {
        let mut h = HeadingPlausibility::new(3);
        h.check(0.0, 1.5, 0.1); // init
        // Three consecutive glitches → SensorFailure
        assert!(matches!(
            h.check(2.0, 1.5, 0.1),
            PlausibilityResult::Glitch(_)
        ));
        assert!(matches!(
            h.check(2.0, 1.5, 0.1),
            PlausibilityResult::Glitch(_)
        ));
        assert!(matches!(
            h.check(2.0, 1.5, 0.1),
            PlausibilityResult::SensorFailure
        ));
    }

    #[test]
    fn plausibility_glitch_count_resets_on_ok() {
        let mut h = HeadingPlausibility::new(3);
        h.check(0.0, 1.5, 0.1); // init
        // Two glitches
        assert!(matches!(
            h.check(2.0, 1.5, 0.1),
            PlausibilityResult::Glitch(_)
        ));
        assert!(matches!(
            h.check(2.0, 1.5, 0.1),
            PlausibilityResult::Glitch(_)
        ));
        // Return to plausible heading → resets count
        assert!(matches!(h.check(0.05, 1.5, 0.1), PlausibilityResult::Ok(_)));
        // Two more glitches — still no failure (counter was reset)
        assert!(matches!(
            h.check(2.0, 1.5, 0.1),
            PlausibilityResult::Glitch(_)
        ));
        assert!(matches!(
            h.check(2.0, 1.5, 0.1),
            PlausibilityResult::Glitch(_)
        ));
        // Third → now failure
        assert!(matches!(
            h.check(2.0, 1.5, 0.1),
            PlausibilityResult::SensorFailure
        ));
    }

    #[test]
    fn plausibility_reset_clears_state() {
        let mut h = HeadingPlausibility::new(3);
        h.check(0.0, 1.5, 0.1);
        h.check(2.0, 1.5, 0.1); // glitch
        h.reset();
        // After reset, next sample is "first" again → Ok
        match h.check(2.0, 1.5, 0.1) {
            PlausibilityResult::Ok(v) => assert!((v - 2.0).abs() < 1e-10),
            other => panic!("expected Ok after reset, got {other:?}"),
        }
    }

    #[test]
    fn plausibility_pi_boundary_wrap() {
        // Heading near ±π boundary: 3.1 → -3.1 is a small change (~0.08 rad),
        // not a 6.2 rad jump.
        let mut h = HeadingPlausibility::new(3);
        h.check(3.1, 1.5, 0.1); // init near +π
        // -3.1 is only ~0.083 rad away (wrapping) → 0.83 rad/s, under max 1.5
        match h.check(-3.1, 1.5, 0.1) {
            PlausibilityResult::Ok(v) => assert!((v - (-3.1)).abs() < 1e-10),
            other => panic!("expected Ok at ±π boundary, got {other:?}"),
        }
    }

    // ── sensor_quality ──────────────────────────────────────────────────────

    #[test]
    fn quality_fresh_sensor_is_one() {
        assert!((sensor_quality(0.0, 0.5) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn quality_at_half_life() {
        assert!((sensor_quality(0.5, 0.5) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn quality_stale_is_low() {
        let q = sensor_quality(2.0, 0.5); // 4 half-lives
        assert!(q < 0.1, "expected < 0.1, got {q}");
        assert!(q > 0.0);
    }

    #[test]
    fn quality_nan_returns_zero() {
        assert_eq!(sensor_quality(f64::NAN, 0.5), 0.0);
        assert_eq!(sensor_quality(0.0, f64::NAN), 0.0);
    }

    #[test]
    fn quality_negative_age_returns_one() {
        assert!((sensor_quality(-1.0, 0.5) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn quality_disabled_when_half_life_zero() {
        assert!((sensor_quality(100.0, 0.0) - 1.0).abs() < 1e-10);
    }

    // ── validate_yaw_rate ──────────────────────────────────────────────────

    #[test]
    fn yaw_rate_within_bound() {
        assert_eq!(validate_yaw_rate(Some(0.5), 0.8), Some(0.5));
    }

    #[test]
    fn yaw_rate_exceeds_bound() {
        assert_eq!(validate_yaw_rate(Some(1.0), 0.8), None);
    }

    #[test]
    fn yaw_rate_none_passthrough() {
        assert_eq!(validate_yaw_rate(None, 0.8), None);
    }

    #[test]
    fn yaw_rate_nan_returns_none() {
        assert_eq!(validate_yaw_rate(Some(f64::NAN), 0.8), None);
    }

    #[test]
    fn yaw_rate_negative_within_bound() {
        assert_eq!(validate_yaw_rate(Some(-0.5), 0.8), Some(-0.5));
    }

    #[test]
    fn yaw_rate_exact_bound_passes() {
        assert_eq!(validate_yaw_rate(Some(0.8), 0.8), Some(0.8));
    }

    // ── Property-based tests ────────────────────────────────────────────────

    proptest! {
        #[test]
        fn normalize_always_in_range(a in -1e6_f64..1e6_f64) {
            let r = normalize_angle(a);
            prop_assert!((-PI..=PI).contains(&r), "normalize_angle({a}) = {r}");
        }

        #[test]
        fn normalize_idempotent(a in -1e6_f64..1e6_f64) {
            let once = normalize_angle(a);
            let twice = normalize_angle(once);
            prop_assert!((once - twice).abs() < 1e-12);
        }

        #[test]
        fn pid_output_always_clamped(
            error in -PI..PI,
            d_error in -10.0_f64..10.0_f64,
            gain_p in 0.0_f64..10.0_f64,
            gain_i in 0.0_f64..2.0_f64,
            gain_d in 0.0_f64..10.0_f64,
            dead_zone in 0.0_f64..0.1_f64,
            max_rudder in 0.01_f64..PI,
        ) {
            let mut pid = PidController::new(1.0);
            let cfg = PidConfig {
                gain_p, gain_i, gain_d, dead_zone_rad: dead_zone, max_rudder_rad: max_rudder,
            };
            let r = pid.compute(error, d_error, 1.0, &cfg);
            prop_assert!(
                r >= -max_rudder - 1e-10 && r <= max_rudder + 1e-10,
                "rudder {r} out of [{}, {}]", -max_rudder, max_rudder
            );
        }

        #[test]
        fn dead_zone_always_zero(
            error in -0.05_f64..0.05_f64,
            d_error in -1.0_f64..1.0_f64,
        ) {
            let mut pid = PidController::new(1.0);
            let cfg = PidConfig {
                gain_p: 1.0, gain_i: 0.05, gain_d: 0.5,
                dead_zone_rad: 0.1, // larger than error range
                max_rudder_rad: PI,
            };
            let r = pid.compute(error, d_error, 1.0, &cfg);
            prop_assert_eq!(r, 0.0);
        }

        #[test]
        fn rate_limit_output_between_current_and_target(
            current in -1.0_f64..1.0_f64,
            target in -1.0_f64..1.0_f64,
            max_rate in 0.01_f64..1.0_f64,
        ) {
            let r = rate_limit(current, target, max_rate, 1.0);
            let lo = current.min(target);
            let hi = current.max(target);
            prop_assert!(
                r >= lo - 1e-10 && r <= hi + 1e-10,
                "rate_limit({current}, {target}, {max_rate}) = {r} not in [{lo}, {hi}]"
            );
        }

        #[test]
        fn sensor_quality_always_in_unit_range(
            age in 0.0_f64..100.0_f64,
            half_life in 0.01_f64..10.0_f64,
        ) {
            let q = sensor_quality(age, half_life);
            prop_assert!((0.0..=1.0).contains(&q), "quality({age}, {half_life}) = {q}");
        }

        #[test]
        fn sensor_quality_monotonically_decreasing(
            age1 in 0.0_f64..50.0_f64,
            age2 in 0.0_f64..50.0_f64,
            half_life in 0.01_f64..10.0_f64,
        ) {
            let (a, b) = if age1 <= age2 { (age1, age2) } else { (age2, age1) };
            let q_a = sensor_quality(a, half_life);
            let q_b = sensor_quality(b, half_life);
            prop_assert!(
                q_a >= q_b - 1e-10,
                "quality({a})={q_a} < quality({b})={q_b}, should be monotone"
            );
        }
    }
}
