/// Autopilot state machine types and configuration.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

// ─── Mode ─────────────────────────────────────────────────────────────────────

/// Active control algorithm.
///
/// Stable modes are always available. Experimental modes are gated behind the
/// `experimental` Cargo feature — they are excluded from release builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutopilotMode {
    /// Heading hold using `navigation.headingMagnetic`.
    Compass,
    /// Apparent wind angle hold using `environment.wind.angleApparent`.
    Wind,
    /// True wind angle hold using `environment.wind.angleTrue`.
    /// More stable than AWA at varying speeds.
    #[cfg(feature = "experimental")]
    WindTrue,
    /// Route following via cascaded LOS guidance.
    /// Outer loop: XTE → heading correction. Inner loop: PID on desired heading.
    #[cfg(feature = "experimental")]
    Route,
}

impl AutopilotMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AutopilotMode::Compass => "compass",
            AutopilotMode::Wind => "wind",
            #[cfg(feature = "experimental")]
            AutopilotMode::WindTrue => "wind_true",
            #[cfg(feature = "experimental")]
            AutopilotMode::Route => "route",
        }
    }

    /// The SK path this mode reads as its primary sensor input.
    pub fn sensor_path(&self) -> &'static str {
        match self {
            AutopilotMode::Compass => "navigation.headingMagnetic",
            AutopilotMode::Wind => "environment.wind.angleApparent",
            #[cfg(feature = "experimental")]
            AutopilotMode::WindTrue => "environment.wind.angleTrue",
            #[cfg(feature = "experimental")]
            AutopilotMode::Route => "navigation.course.calcValues.bearingTrackTrue",
        }
    }

    /// The SK path this mode writes its target to (for WS clients).
    pub fn target_path(&self) -> &'static str {
        match self {
            AutopilotMode::Compass => "steering.autopilot.target.headingMagnetic",
            AutopilotMode::Wind => "steering.autopilot.target.windAngleApparent",
            #[cfg(feature = "experimental")]
            AutopilotMode::WindTrue => "steering.autopilot.target.windAngleTrue",
            #[cfg(feature = "experimental")]
            AutopilotMode::Route => "steering.autopilot.target.headingTrue",
        }
    }
}

impl std::str::FromStr for AutopilotMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "compass" => Ok(AutopilotMode::Compass),
            "wind" => Ok(AutopilotMode::Wind),
            #[cfg(feature = "experimental")]
            "wind_true" => Ok(AutopilotMode::WindTrue),
            #[cfg(feature = "experimental")]
            "route" => Ok(AutopilotMode::Route),
            other => Err(format!("unknown autopilot mode: {other}")),
        }
    }
}

// ─── Runtime state ────────────────────────────────────────────────────────────

/// Mutable runtime state — held behind `Arc<RwLock<AutopilotState>>`.
pub struct AutopilotState {
    /// Whether the autopilot is currently steering.
    pub enabled: bool,
    /// Active control algorithm.
    pub mode: AutopilotMode,
    /// Target value in radians. `None` = no target set.
    pub target_rad: Option<f64>,
    /// Dodge mode: temporary offset in radians. `None` = not in dodge mode.
    pub dodge_offset_rad: Option<f64>,
    /// Last commanded rudder angle (radians).
    pub last_rudder_rad: f64,
    /// Error from previous tick — used for D-term finite-difference fallback.
    pub last_error_rad: f64,
    /// Timestamp of the previous control loop tick — used to compute dt.
    pub last_tick_at: Option<Instant>,
    /// Last time a sensor value was received per path.
    pub sensor_last_seen: HashMap<String, Instant>,
    /// Cached sensor values (updated by subscription callback).
    pub sensor_values: HashMap<String, f64>,
    /// Actual rudder angle from hardware sensor (NMEA source, not autopilot output).
    /// `None` if no rudder feedback sensor is available.
    pub actual_rudder_rad: Option<f64>,
    /// When the last rudder feedback was received.
    pub actual_rudder_last_seen: Option<Instant>,
}

impl AutopilotState {
    pub fn new(mode: AutopilotMode) -> Self {
        AutopilotState {
            enabled: false,
            mode,
            target_rad: None,
            dodge_offset_rad: None,
            last_rudder_rad: 0.0,
            last_error_rad: 0.0,
            last_tick_at: None,
            sensor_last_seen: HashMap::new(),
            sensor_values: HashMap::new(),
            actual_rudder_rad: None,
            actual_rudder_last_seen: None,
        }
    }

    /// Update sensor data from an incoming delta value.
    pub fn update_sensor(&mut self, path: &str, value: f64) {
        self.sensor_values.insert(path.to_string(), value);
        self.sensor_last_seen
            .insert(path.to_string(), Instant::now());
    }

    /// Get the current sensor reading for the active mode, if available.
    pub fn current_sensor(&self) -> Option<f64> {
        self.sensor_values.get(self.mode.sensor_path()).copied()
    }

    /// Check whether the active mode's sensor has timed out.
    pub fn sensor_timed_out(&self, timeout_secs: u64) -> bool {
        match self.sensor_last_seen.get(self.mode.sensor_path()) {
            Some(last) => last.elapsed().as_secs() > timeout_secs,
            None => true,
        }
    }
}

// ─── Configuration ────────────────────────────────────────────────────────────

/// Static plugin configuration — parsed from `[[plugins]]` TOML entry.
///
/// All angles in radians, speeds in m/s.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotConfig {
    // ── Identity ─────────────────────────────────────────────────────────────
    /// Device ID used in the V2 autopilot API. Default: `"default"`.
    #[serde(default = "default_device_id")]
    pub device_id: String,
    /// Initial mode on plugin start. Default: `"compass"`.
    #[serde(default = "default_mode")]
    pub initial_mode: String,

    // ── PID gains ────────────────────────────────────────────────────────────
    /// Proportional gain.
    #[serde(default = "default_gain_p")]
    pub gain_p: f64,
    /// Integral gain. Eliminates steady-state error from wind/current.
    #[serde(default = "default_gain_i")]
    pub gain_i: f64,
    /// Derivative gain. Dampens oscillation and overshoot.
    #[serde(default = "default_gain_d")]
    pub gain_d: f64,
    /// Maximum integral accumulation (radians). Prevents windup.
    #[serde(default = "default_integral_limit")]
    pub integral_limit: f64,
    /// Dead zone: no rudder correction within ±dead_zone_rad. Prevents hunting.
    #[serde(default = "default_dead_zone_rad")]
    pub dead_zone_rad: f64,
    /// Maximum rudder angle commanded (radians).
    #[serde(default = "default_max_rudder_rad")]
    pub max_rudder_rad: f64,

    // ── Gain scheduling ──────────────────────────────────────────────────────
    /// Speed at which default gains are correct (m/s). Gains scale with
    /// `speed / speed_nominal` (Nomoto principle). 0 = disabled.
    #[serde(default = "default_speed_nominal_mps")]
    pub speed_nominal_mps: f64,

    // ── Heel compensation ────────────────────────────────────────────────────
    /// Feedforward gain for heel (roll) angle. Counters weather helm.
    /// Negative = counter weather helm (typical: −0.3 to −0.8).
    #[serde(default = "default_heel_gain")]
    pub heel_gain: f64,

    // ── Rudder rate limiting ─────────────────────────────────────────────────
    /// Maximum rudder movement speed (rad/s). Models physical actuator limits.
    /// 0 = unlimited.
    #[serde(default = "default_max_rudder_rate")]
    pub max_rudder_rate_rad_per_sec: f64,

    // ── Timing ───────────────────────────────────────────────────────────────
    /// Internal PID computation rate (Hz).
    #[serde(default = "default_control_rate_hz")]
    pub control_rate_hz: f64,
    /// SK store delta emission rate (Hz).
    #[serde(default = "default_output_rate_hz")]
    pub output_rate_hz: f64,
    /// Sensor timeout in seconds — emit alarm and disengage if exceeded.
    #[serde(default = "default_sensor_timeout_secs")]
    pub sensor_timeout_secs: u64,

    // ── Recovery mode ──────────────────────────────────────────────────────
    /// Error threshold (radians) to trigger recovery mode. 0 = disabled.
    #[serde(default = "default_recovery_threshold_rad")]
    pub recovery_threshold_rad: f64,
    /// Maximum ticks in recovery mode before automatic exit.
    #[serde(default = "default_recovery_max_ticks")]
    pub recovery_max_ticks: u32,
    /// Gain multiplier during recovery (P and D only, I disabled).
    #[serde(default = "default_recovery_gain_factor")]
    pub recovery_gain_factor: f64,

    // ── Gust response ───────────────────────────────────────────────────────
    /// Feedforward gain for wind speed rate-of-change.
    /// Applies preemptive rudder when a gust is detected.
    /// Negative = bear away from gust (typical for sailing yacht).
    #[serde(default = "default_gust_gain")]
    pub gust_gain: f64,
    /// Minimum d(AWS)/dt (m/s²) to trigger gust response.
    #[serde(default = "default_gust_threshold")]
    pub gust_threshold_mps_per_sec: f64,

    // ── Route mode (experimental) ────────────────────────────────────────────
    /// XTE (cross-track error) gain for route mode LOS guidance (rad/m).
    /// Equivalent to `1 / lookahead_distance_m`.
    #[serde(default = "default_xte_gain")]
    pub xte_gain: f64,
    /// Lookahead distance (metres) for cascaded route LOS guidance.
    /// Outer loop: heading_correction = atan(XTE / lookahead_distance).
    #[serde(default = "default_route_lookahead_m")]
    pub route_lookahead_m: f64,

    // ── Wind filtering ───────────────────────────────────────────────────────
    /// Circular low-pass filter smoothing factor for wind angle (0 < α ≤ 1).
    /// Smaller values smooth more aggressively.
    #[serde(default = "default_wind_filter_alpha")]
    pub wind_filter_alpha: f64,

    // ── Rudder feedback monitoring ──────────────────────────────────────────
    /// Mismatch threshold between commanded and actual rudder angle (radians).
    /// When `|commanded − actual| > threshold` for `rudder_feedback_timeout_ticks`
    /// consecutive ticks, an alarm is emitted. 0 = disabled.
    /// Requires a rudder feedback sensor (NMEA RSA / PGN 127245).
    #[serde(default = "default_rudder_feedback_threshold_rad")]
    pub rudder_feedback_threshold_rad: f64,
    /// Consecutive mismatch ticks before alarm is raised.
    #[serde(default = "default_rudder_feedback_timeout_ticks")]
    pub rudder_feedback_timeout_ticks: u32,

    // ── Heading plausibility ────────────────────────────────────────────────
    /// Maximum plausible heading rate of change (rad/s).
    /// Deltas exceeding this are classified as sensor glitches.
    /// ~86°/s — well above any real vessel turn rate.
    #[serde(default = "default_max_heading_rate_rad_per_sec")]
    pub max_heading_rate_rad_per_sec: f64,
    /// Maximum plausible yaw rate from ROT sensor (rad/s).
    /// Values exceeding this are discarded (fallback to finite-difference D-term).
    /// ~46°/s — max realistic rate of turn.
    #[serde(default = "default_max_yaw_rate_rad_per_sec")]
    pub max_yaw_rate_rad_per_sec: f64,
    /// Consecutive heading glitches before autopilot disengages.
    /// At 10 Hz, 3 ticks = 300 ms of persistent glitch → sensor failure.
    #[serde(default = "default_heading_glitch_max_count")]
    pub heading_glitch_max_count: u32,
    /// Half-life for D-term quality decay (seconds).
    /// D-gain is scaled by `2^(-sensor_age / half_life)`.
    /// 0 = disabled (D-gain always full).
    #[serde(default = "default_dterm_quality_half_life_secs")]
    pub dterm_quality_half_life_secs: f64,
}

fn default_device_id() -> String {
    "default".to_string()
}
fn default_mode() -> String {
    "compass".to_string()
}
fn default_gain_p() -> f64 {
    1.0
}
fn default_gain_i() -> f64 {
    0.05
}
fn default_gain_d() -> f64 {
    0.3
}
fn default_integral_limit() -> f64 {
    0.5
}
fn default_dead_zone_rad() -> f64 {
    0.01745 // ~1°
}
fn default_max_rudder_rad() -> f64 {
    std::f64::consts::FRAC_PI_6 // 30°
}
fn default_speed_nominal_mps() -> f64 {
    2.5 // ~5 knots — typical cruising speed for a 35ft yacht
}
fn default_heel_gain() -> f64 {
    -0.5 // negative = counter weather helm
}
fn default_max_rudder_rate() -> f64 {
    0.09 // ~5°/s — typical hydraulic actuator
}
fn default_control_rate_hz() -> f64 {
    10.0
}
fn default_output_rate_hz() -> f64 {
    1.0
}
fn default_sensor_timeout_secs() -> u64 {
    10
}
fn default_recovery_threshold_rad() -> f64 {
    0.35 // ~20° — large enough that normal corrections handle smaller errors
}
fn default_recovery_max_ticks() -> u32 {
    15 // 1.5 s at 10 Hz
}
fn default_recovery_gain_factor() -> f64 {
    2.0
}
fn default_gust_gain() -> f64 {
    -0.02 // negative = bear away from gust (counter weather helm increase)
}
fn default_gust_threshold() -> f64 {
    3.0 // m/s² — only respond to rapid wind speed changes
}
fn default_xte_gain() -> f64 {
    0.01 // rad/m — equivalent to 100 m lookahead distance
}
fn default_route_lookahead_m() -> f64 {
    100.0 // metres — standard LOS lookahead for coastal navigation
}
fn default_wind_filter_alpha() -> f64 {
    0.15 // τ ≈ 0.6 s at 10 Hz: smooths gusts while tracking genuine shifts
}
fn default_rudder_feedback_threshold_rad() -> f64 {
    0.087 // ~5° — mismatch between commanded and actual rudder
}
fn default_rudder_feedback_timeout_ticks() -> u32 {
    30 // 3 s at 10 Hz — consecutive mismatch ticks before alarm
}
fn default_max_heading_rate_rad_per_sec() -> f64 {
    1.5 // ~86°/s — well above any real vessel turn rate
}
fn default_max_yaw_rate_rad_per_sec() -> f64 {
    0.8 // ~46°/s — max plausible rate of turn
}
fn default_heading_glitch_max_count() -> u32 {
    3 // 3 ticks at 10 Hz = 300 ms → sensor failure
}
fn default_dterm_quality_half_life_secs() -> f64 {
    0.5 // D-gain halved every 0.5 s of staleness
}

impl Default for AutopilotConfig {
    fn default() -> Self {
        AutopilotConfig {
            device_id: default_device_id(),
            initial_mode: default_mode(),
            gain_p: default_gain_p(),
            gain_i: default_gain_i(),
            gain_d: default_gain_d(),
            integral_limit: default_integral_limit(),
            dead_zone_rad: default_dead_zone_rad(),
            max_rudder_rad: default_max_rudder_rad(),
            speed_nominal_mps: default_speed_nominal_mps(),
            heel_gain: default_heel_gain(),
            max_rudder_rate_rad_per_sec: default_max_rudder_rate(),
            control_rate_hz: default_control_rate_hz(),
            output_rate_hz: default_output_rate_hz(),
            sensor_timeout_secs: default_sensor_timeout_secs(),
            recovery_threshold_rad: default_recovery_threshold_rad(),
            recovery_max_ticks: default_recovery_max_ticks(),
            recovery_gain_factor: default_recovery_gain_factor(),
            gust_gain: default_gust_gain(),
            gust_threshold_mps_per_sec: default_gust_threshold(),
            xte_gain: default_xte_gain(),
            route_lookahead_m: default_route_lookahead_m(),
            wind_filter_alpha: default_wind_filter_alpha(),
            rudder_feedback_threshold_rad: default_rudder_feedback_threshold_rad(),
            rudder_feedback_timeout_ticks: default_rudder_feedback_timeout_ticks(),
            max_heading_rate_rad_per_sec: default_max_heading_rate_rad_per_sec(),
            max_yaw_rate_rad_per_sec: default_max_yaw_rate_rad_per_sec(),
            heading_glitch_max_count: default_heading_glitch_max_count(),
            dterm_quality_half_life_secs: default_dterm_quality_half_life_secs(),
        }
    }
}

impl AutopilotConfig {
    /// Build a `PidConfig` from the autopilot config (convenience for mode dispatch).
    pub fn pid_config(&self) -> crate::pd::PidConfig {
        crate::pd::PidConfig {
            gain_p: self.gain_p,
            gain_i: self.gain_i,
            gain_d: self.gain_d,
            dead_zone_rad: self.dead_zone_rad,
            max_rudder_rad: self.max_rudder_rad,
        }
    }
}
