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
    /// ✅ Stable — heading hold using `navigation.headingMagnetic`.
    Compass,
    /// ✅ Stable — apparent wind angle hold using `environment.wind.angleApparent`.
    Wind,
    /// 🧪 Experimental — route following via LOS guidance.
    /// Uses `navigation.course.nextPoint.bearing` + `navigation.crossTrackError`.
    /// Enable with `--features autopilot-experimental` (via signalk-server).
    #[cfg(feature = "experimental")]
    Route,
}

impl AutopilotMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AutopilotMode::Compass => "compass",
            AutopilotMode::Wind => "wind",
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
            AutopilotMode::Route => "navigation.course.nextPoint.bearing",
        }
    }

    /// The SK path this mode writes its target to (for WS clients).
    pub fn target_path(&self) -> &'static str {
        match self {
            AutopilotMode::Compass => "steering.autopilot.target.headingMagnetic",
            AutopilotMode::Wind => "steering.autopilot.target.windAngleApparent",
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
    /// Error from previous tick — used for D-term.
    pub last_error_rad: f64,
    /// Timestamp of the previous control loop tick — used to compute dt for D-term.
    pub last_tick_at: Option<Instant>,
    /// Last time a sensor value was received per path.
    pub sensor_last_seen: HashMap<String, Instant>,
    /// Cached sensor values (updated by subscription callback).
    pub sensor_values: HashMap<String, f64>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotConfig {
    /// Device ID used in the V2 autopilot API. Default: `"default"`.
    #[serde(default = "default_device_id")]
    pub device_id: String,
    /// Initial mode on plugin start. Default: `"compass"`.
    #[serde(default = "default_mode")]
    pub initial_mode: String,
    /// Proportional gain. Default: 1.0.
    #[serde(default = "default_gain_p")]
    pub gain_p: f64,
    /// Derivative gain. Default: 0.3.
    /// Increase to dampen oscillation; decrease if response feels sluggish.
    #[serde(default = "default_gain_d")]
    pub gain_d: f64,
    /// Dead zone: no rudder correction within ±dead_zone_rad. Default: ~1° (0.01745 rad).
    #[serde(default = "default_dead_zone_rad")]
    pub dead_zone_rad: f64,
    /// Maximum rudder angle commanded (radians). Default: 30° (π/6).
    #[serde(default = "default_max_rudder_rad")]
    pub max_rudder_rad: f64,
    /// Sensor timeout in seconds — emit alarm and disengage if exceeded. Default: 10.
    #[serde(default = "default_sensor_timeout_secs")]
    pub sensor_timeout_secs: u64,
    /// Control loop interval in milliseconds. Default: 1000 (1 Hz).
    #[serde(default = "default_loop_interval_ms")]
    pub loop_interval_ms: u64,
    /// XTE (cross-track error) gain for route mode LOS guidance (rad/m).
    /// Converts cross-track error to a heading correction:
    ///   `desired_heading = BTW + xte_gain * XTE_meters`
    /// Equivalent to `1 / lookahead_distance_m`. Default: 0.01 (100 m lookahead).
    #[serde(default = "default_xte_gain")]
    pub xte_gain: f64,
    /// Low-pass filter smoothing factor for wind angle in wind mode (0 < α ≤ 1).
    /// Smaller values smooth more aggressively. Default: 0.3 (τ ≈ 2 s at 1 Hz).
    #[serde(default = "default_wind_filter_alpha")]
    pub wind_filter_alpha: f64,
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
fn default_gain_d() -> f64 {
    0.3
}
fn default_dead_zone_rad() -> f64 {
    0.01745 // ~1°
}
fn default_max_rudder_rad() -> f64 {
    std::f64::consts::FRAC_PI_6 // 30°
}
fn default_sensor_timeout_secs() -> u64 {
    10
}
fn default_loop_interval_ms() -> u64 {
    1000
}
fn default_xte_gain() -> f64 {
    0.01 // rad/m — equivalent to 100 m lookahead distance
}
fn default_wind_filter_alpha() -> f64 {
    0.3 // τ ≈ 2.3 s at 1 Hz: smooths gusts while tracking genuine shifts
}

impl Default for AutopilotConfig {
    fn default() -> Self {
        AutopilotConfig {
            device_id: default_device_id(),
            initial_mode: default_mode(),
            gain_p: default_gain_p(),
            gain_d: default_gain_d(),
            dead_zone_rad: default_dead_zone_rad(),
            max_rudder_rad: default_max_rudder_rad(),
            sensor_timeout_secs: default_sensor_timeout_secs(),
            loop_interval_ms: default_loop_interval_ms(),
            xte_gain: default_xte_gain(),
            wind_filter_alpha: default_wind_filter_alpha(),
        }
    }
}
