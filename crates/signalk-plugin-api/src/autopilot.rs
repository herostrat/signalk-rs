/// AutopilotProvider trait — the interface for autopilot hardware/software drivers.
///
/// Plugins that control an autopilot implement this trait and register via
/// `PluginContext::register_autopilot_provider()`. The server's V2 autopilot API
/// (`/signalk/v2/api/vessels/self/autopilots/`) delegates all commands to registered
/// providers.
///
/// # Device ID
///
/// Each provider has a unique device ID (e.g. `"default"`, `"raymarine-e70310"`).
/// The server maintains a "default" pointer — the first registered provider becomes
/// the default automatically. Use `/_providers/_default/{id}` to change it.
///
/// # Required methods
///
/// All methods MUST be implemented. If a feature is not supported, return
/// `Err(PluginError::not_found("tack not supported in this mode"))` rather than
/// silently ignoring the call.
///
/// # Emitting state
///
/// Providers are expected to emit `steering.autopilot.*` deltas via
/// `ctx.handle_message()` when state changes (on engage, mode change, target
/// change, etc.) so that the SK store reflects current autopilot state.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PluginError;

// ─── Data types ──────────────────────────────────────────────────────────────

/// Full autopilot device state — returned by `GET /autopilots/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotData {
    /// Device power/operational state: `"enabled"`, `"disabled"`, or `"offline"`.
    pub state: String,
    /// Active control algorithm: `"compass"`, `"wind"`, `"route"`, etc.
    pub mode: String,
    /// Current target in radians (heading or wind angle, depending on mode).
    /// `None` when not engaged or no target set.
    pub target: Option<f64>,
    /// Whether the autopilot is currently engaged (actively steering).
    pub engaged: bool,
    /// Capabilities of this device.
    pub options: AutopilotOptions,
}

/// Capabilities exposed by an autopilot provider.
///
/// Spec: https://demo.signalk.org/documentation/develop/rest-api/autopilot_api.html
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutopilotOptions {
    /// Available device states (e.g. `["enabled", "disabled"]`).
    pub state: Vec<String>,
    /// Supported control modes (e.g. `["compass", "wind", "route"]`).
    pub mode: Vec<String>,
    /// Available actions with current availability status.
    pub actions: Vec<AutopilotAction>,
}

/// An autopilot action (tack, gybe, dodge, etc.) with availability status.
///
/// Normalised action IDs per spec: `"dodge"`, `"tack"`, `"gybe"`,
/// `"courseCurrentPoint"`, `"courseNextPoint"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotAction {
    /// Normalised action identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Whether the action is currently available in the device's state.
    pub available: bool,
}

/// Tack/gybe direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TackDirection {
    Port,
    Starboard,
}

impl TackDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            TackDirection::Port => "port",
            TackDirection::Starboard => "starboard",
        }
    }
}

// ─── AutopilotProvider trait ──────────────────────────────────────────────────

/// Autopilot device interface. Implement this to expose an autopilot to the
/// SignalK V2 autopilot API.
///
/// All methods use `&self` — the provider is wrapped in `Arc<dyn AutopilotProvider>`
/// for safe sharing across async tasks.
#[async_trait]
pub trait AutopilotProvider: Send + Sync + 'static {
    /// Unique device identifier.
    ///
    /// This ID appears in the V2 API URL: `/autopilots/{device_id}/...`
    fn device_id(&self) -> &str;

    /// Return full device state (state, mode, target, engaged, options).
    async fn get_data(&self) -> Result<AutopilotData, PluginError>;

    // ── State (enabled / disabled / offline) ──────────────────────────

    /// Get device state: `"enabled"`, `"disabled"`, or `"offline"`.
    async fn get_state(&self) -> Result<String, PluginError>;

    /// Set device state: `"enabled"` or `"disabled"`.
    async fn set_state(&self, state: &str) -> Result<(), PluginError>;

    // ── Mode (compass / wind / route / …) ─────────────────────────────

    /// Get current control mode (e.g. `"compass"`, `"wind"`, `"route"`).
    async fn get_mode(&self) -> Result<String, PluginError>;

    /// Set control mode. Implementations should validate against supported modes.
    async fn set_mode(&self, mode: &str) -> Result<(), PluginError>;

    // ── Target heading / angle ─────────────────────────────────────────

    /// Get current target value in radians.
    async fn get_target(&self) -> Result<Option<f64>, PluginError>;

    /// Set target value in radians (heading or wind angle, depending on mode).
    async fn set_target(&self, value_rad: f64) -> Result<(), PluginError>;

    /// Adjust current target by a relative offset in radians.
    async fn adjust_target(&self, delta_rad: f64) -> Result<(), PluginError>;

    // ── Engagement ────────────────────────────────────────────────────

    /// Engage the autopilot (activate steering).
    async fn engage(&self) -> Result<(), PluginError>;

    /// Disengage the autopilot (return to standby).
    async fn disengage(&self) -> Result<(), PluginError>;

    // ── Maneuvers ─────────────────────────────────────────────────────

    /// Execute a tack maneuver to port or starboard.
    ///
    /// Implementations that do not support tacking should return
    /// `Err(PluginError::not_found("tack not supported"))`.
    async fn tack(&self, direction: TackDirection) -> Result<(), PluginError>;

    /// Execute a gybe maneuver to port or starboard.
    ///
    /// Implementations that do not support gybing should return
    /// `Err(PluginError::not_found("gybe not supported"))`.
    async fn gybe(&self, direction: TackDirection) -> Result<(), PluginError>;

    // ── Dodge mode ────────────────────────────────────────────────────

    /// Activate dodge mode (`Some(offset_rad)`) or deactivate it (`None`).
    ///
    /// Dodge is a temporary heading offset for obstacle avoidance. Pass
    /// `None` to return to the original target.
    async fn dodge(&self, offset_rad: Option<f64>) -> Result<(), PluginError>;

    // ── Course operations ──────────────────────────────────────────

    /// Start steering to the current course destination.
    ///
    /// Sets the autopilot to an appropriate GPS/route mode and engages.
    /// Requires an active course (nextPoint) in the navigation state.
    ///
    /// Implementations that do not support course following should return
    /// `Err(PluginError::bad_request("course following not supported"))`.
    async fn course_current_point(&self) -> Result<(), PluginError>;

    /// React to waypoint advancement on the active route.
    ///
    /// For software autopilots: ensure the route mode remains active.
    /// For hardware autopilots: send the "next waypoint" command to the device.
    ///
    /// Note: The server advances the course waypoint before calling this method.
    async fn course_next_point(&self) -> Result<(), PluginError>;
}
