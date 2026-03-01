//! Software autopilot plugin for signalk-rs.
//!
//! Registers as a SignalK V2 `AutopilotProvider` and implements a PD heading
//! controller. All sensor input and control output travels through the SK store —
//! no direct hardware access.
//!
//! # Data flow
//! ```text
//! SK navigation.headingMagnetic  ──► [subscription callback]
//! SK environment.wind.angleApparent                         │
//! SK navigation.rateOfTurn (optional)                       ▼
//!                                              AutopilotState (Arc<RwLock<>>)
//!                                                           │
//!                                              [control loop, 1 Hz]
//!                                                           │
//!                            ┌──────────────────────────────┴──────────────┐
//!                        Compass                          Wind           Route
//!                   heading::compute()            LowPassFilter +    LOS guidance +
//!                   (yaw_rate D-term)              PD controller     heading::compute()
//!                            └──────────────────────────────┬──────────────┘
//!                                                           │
//!                                                           ▼
//!                                      SK steering.rudderAngle  ──► hardware driver
//!                                      SK steering.autopilot.{state,mode}
//! ```
//!
//! # Modes
//! - **compass** — heading hold using `navigation.headingMagnetic`
//! - **wind**    — wind angle hold using `environment.wind.angleApparent`,
//!   with configurable low-pass filtering to smooth gusts
//! - **route**   — bearing hold using Line-of-Sight (LOS) guidance:
//!   `desired_heading = BTW + xte_gain × XTE`, then heading PD
//!
//! # Configuration example (signalk-rs.toml)
//! ```toml
//! [[plugins]]
//! id = "autopilot"
//! enabled = true
//! config = { device_id = "default", gain_p = 1.0, gain_d = 0.3, xte_gain = 0.01 }
//! ```
pub mod filter;
pub mod modes;
pub mod pd;
pub(crate) mod provider;
pub mod state;

use async_trait::async_trait;
use filter::LowPassFilter;
use provider::ProviderHandle;
use signalk_plugin_api::{
    Plugin, PluginContext, PluginError, PluginMetadata, SubscriptionHandle, SubscriptionSpec,
    delta_callback,
};
use signalk_types::{
    Delta, Notification, NotificationMethod, NotificationState, PathValue, Source, Subscription,
    Update,
};
use state::{AutopilotConfig, AutopilotMode, AutopilotState};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{info, warn};

// ─── Plugin struct ────────────────────────────────────────────────────────────

pub struct AutopilotPlugin {
    config: AutopilotConfig,
    state: Arc<RwLock<AutopilotState>>,
    subscription_handle: Option<SubscriptionHandle>,
    tick_handle: Option<tokio::task::AbortHandle>,
}

impl AutopilotPlugin {
    pub fn new() -> Self {
        let config = AutopilotConfig::default();
        let mode = config
            .initial_mode
            .parse::<AutopilotMode>()
            .unwrap_or(AutopilotMode::Compass);
        AutopilotPlugin {
            config,
            state: Arc::new(RwLock::new(AutopilotState::new(mode))),
            subscription_handle: None,
            tick_handle: None,
        }
    }
}

impl Default for AutopilotPlugin {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Plugin trait ─────────────────────────────────────────────────────────────

#[async_trait]
impl Plugin for AutopilotPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "autopilot",
            "Autopilot",
            "Software autopilot — PD controller, compass/wind/route modes",
            "0.2.0",
        )
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        self.config = if config.is_null() || config == serde_json::json!({}) {
            AutopilotConfig::default()
        } else {
            serde_json::from_value(config).unwrap_or_default()
        };

        let mode = self
            .config
            .initial_mode
            .parse::<AutopilotMode>()
            .unwrap_or(AutopilotMode::Compass);
        *self.state.write().await = AutopilotState::new(mode);

        ctx.set_status("Standby");

        // ── Subscribe to sensor paths ──────────────────────────────────────────
        let state_cb = Arc::clone(&self.state);
        let handle = ctx
            .subscribe(
                SubscriptionSpec::self_vessel(vec![
                    Subscription::path("navigation.headingMagnetic"),
                    Subscription::path("navigation.headingTrue"),
                    Subscription::path("navigation.rateOfTurn"),
                    Subscription::path("environment.wind.angleApparent"),
                    Subscription::path("environment.wind.angleTrue"),
                    Subscription::path("navigation.course.nextPoint.bearing"),
                    Subscription::path("navigation.crossTrackError"),
                ]),
                delta_callback(move |delta: Delta| {
                    let state = Arc::clone(&state_cb);
                    tokio::spawn(async move {
                        let mut st = state.write().await;
                        for update in &delta.updates {
                            for pv in &update.values {
                                if let Some(v) = pv.value.as_f64() {
                                    st.update_sensor(&pv.path, v);
                                }
                            }
                        }
                    });
                }),
            )
            .await?;
        self.subscription_handle = Some(handle);

        // ── Control loop ───────────────────────────────────────────────────────
        let state_tick = Arc::clone(&self.state);
        let ctx_tick = Arc::clone(&ctx);
        let cfg = self.config.clone();
        let interval = std::time::Duration::from_millis(cfg.loop_interval_ms);

        let tick_abort = tokio::spawn(async move {
            // Wind angle low-pass filter — smooths gusts while tracking genuine shifts.
            // Reset on mode change (see below) so filter doesn't carry stale state.
            let mut wind_filter = LowPassFilter::new(cfg.wind_filter_alpha);
            let mut prev_mode: Option<AutopilotMode> = None;

            loop {
                tokio::time::sleep(interval).await;

                // ── Read snapshot from shared state ────────────────────────────
                let snapshot = {
                    let st = state_tick.read().await;

                    // Heading: prefer magnetic, fall back to true
                    let heading = st
                        .sensor_values
                        .get("navigation.headingMagnetic")
                        .or_else(|| st.sensor_values.get("navigation.headingTrue"))
                        .copied();

                    (
                        st.enabled,
                        st.mode.clone(),
                        st.target_rad,
                        st.dodge_offset_rad,
                        st.current_sensor(),
                        st.sensor_timed_out(cfg.sensor_timeout_secs),
                        st.last_error_rad,
                        st.last_tick_at,
                        heading,
                        st.sensor_values.get("navigation.crossTrackError").copied(), // _xte: used only in experimental Route mode
                        st.sensor_values.get("navigation.rateOfTurn").copied(),
                    )
                };
                let (
                    enabled,
                    mode,
                    target,
                    dodge,
                    primary_sensor,
                    timed_out,
                    prev_error,
                    last_tick,
                    heading,
                    _xte, // used only in experimental Route mode
                    yaw_rate,
                ) = snapshot;

                // ── Emit autopilot state for WS clients every tick ─────────────
                emit_autopilot_state(
                    &ctx_tick,
                    if enabled { "enabled" } else { "disabled" },
                    mode.as_str(),
                    target,
                    &mode,
                )
                .await;

                if !enabled {
                    continue;
                }

                // ── Detect mode change → reset filters ─────────────────────────
                if prev_mode.as_ref() != Some(&mode) {
                    wind_filter.reset();
                    prev_mode = Some(mode.clone());
                }

                // ── Sensor timeout → alarm and disengage ───────────────────────
                if timed_out {
                    warn!(mode = %mode.as_str(), "Autopilot: sensor timeout");
                    {
                        let mut st = state_tick.write().await;
                        st.enabled = false;
                        st.last_rudder_rad = 0.0;
                        st.last_tick_at = None;
                    }
                    emit_rudder(&ctx_tick, 0.0).await;
                    emit_notification(
                        &ctx_tick,
                        "steering.autopilot.dataTimeout",
                        NotificationState::Alarm,
                        &format!("Autopilot sensor timeout ({} mode)", mode.as_str()),
                    )
                    .await;
                    ctx_tick.set_status("Sensor timeout — disengaged");
                    continue;
                }

                let target_rad = match target {
                    Some(t) => t,
                    None => continue, // no target set
                };
                let effective_target = target_rad + dodge.unwrap_or(0.0);

                // ── Compute dt from last tick ──────────────────────────────────
                let now = Instant::now();
                let dt = match last_tick {
                    Some(t) => now.duration_since(t).as_secs_f64().max(0.001),
                    None => cfg.loop_interval_ms as f64 / 1000.0,
                };

                // ── Mode-specific control dispatch ─────────────────────────────
                let (rudder, new_error) = match mode {
                    // ── Compass: heading hold with optional yaw-rate D-term ────
                    AutopilotMode::Compass => {
                        let current = match heading {
                            Some(h) => h,
                            None => continue, // no heading available
                        };
                        modes::heading::compute(
                            current,
                            effective_target,
                            prev_error,
                            dt,
                            yaw_rate,
                            &cfg,
                        )
                    }

                    // ── Wind: low-pass filtered apparent wind angle hold ───────
                    AutopilotMode::Wind => {
                        let raw = match primary_sensor {
                            Some(v) => v,
                            None => continue, // no wind angle available
                        };
                        let current = wind_filter.update(raw);
                        let error = pd::normalize_angle(effective_target - current);
                        let d_error = match yaw_rate {
                            Some(rate) => -rate,
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

                    // ── Route: LOS guidance (BTW + XTE correction → desired heading)
                    #[cfg(feature = "experimental")]
                    AutopilotMode::Route => {
                        // For LOS we need both heading and BTW
                        let current_heading = match heading {
                            Some(h) => h,
                            None => continue,
                        };
                        let btw = match primary_sensor {
                            Some(v) => v,
                            None => continue, // no bearing-to-waypoint
                        };
                        // LOS correction: positive XTE (to the right) → steer left
                        let xte_m = _xte.unwrap_or(0.0);
                        let correction =
                            (cfg.xte_gain * xte_m).clamp(-cfg.max_rudder_rad, cfg.max_rudder_rad);
                        let desired_heading =
                            pd::normalize_angle(effective_target + btw + correction);
                        modes::heading::compute(
                            current_heading,
                            desired_heading,
                            prev_error,
                            dt,
                            yaw_rate,
                            &cfg,
                        )
                    }
                };

                {
                    let mut st = state_tick.write().await;
                    st.last_rudder_rad = rudder;
                    st.last_error_rad = new_error;
                    st.last_tick_at = Some(now);
                }

                emit_rudder(&ctx_tick, rudder).await;
            }
        })
        .abort_handle();
        self.tick_handle = Some(tick_abort);

        // ── Register as autopilot provider ────────────────────────────────────
        let device_id = self.config.device_id.clone();
        let provider = Arc::new(ProviderHandle {
            device_id,
            state: Arc::clone(&self.state),
        });
        ctx.register_autopilot_provider(provider).await?;

        info!(
            device_id = %self.config.device_id,
            mode = %self.config.initial_mode,
            "Autopilot ready"
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(h) = self.tick_handle.take() {
            h.abort();
        }
        self.subscription_handle = None;
        Ok(())
    }
}

// ─── SK delta helpers ─────────────────────────────────────────────────────────

pub(crate) async fn emit_rudder(ctx: &Arc<dyn PluginContext>, rudder_rad: f64) {
    let delta = Delta::self_vessel(vec![Update::new(
        Source::plugin("autopilot"),
        vec![PathValue::new(
            "steering.rudderAngle",
            serde_json::json!(rudder_rad),
        )],
    )]);
    let _ = ctx.handle_message(delta).await;
}

/// Emit autopilot state to the SK store so WS clients subscribed to
/// `steering.autopilot.*` paths see the current autopilot status.
pub(crate) async fn emit_autopilot_state(
    ctx: &Arc<dyn PluginContext>,
    state_str: &str,
    mode_str: &str,
    target_rad: Option<f64>,
    mode: &AutopilotMode,
) {
    let source = Source::plugin("autopilot");
    let mut values = vec![
        PathValue::new("steering.autopilot.state", serde_json::json!(state_str)),
        PathValue::new("steering.autopilot.mode", serde_json::json!(mode_str)),
        PathValue::new(
            "steering.autopilot.engaged",
            serde_json::json!(state_str == "enabled"),
        ),
    ];
    if let Some(target) = target_rad {
        values.push(PathValue::new(
            mode.target_path(),
            serde_json::json!(target),
        ));
    }
    let delta = Delta::self_vessel(vec![Update::new(source, values)]);
    let _ = ctx.handle_message(delta).await;
}

pub(crate) async fn emit_notification(
    ctx: &Arc<dyn PluginContext>,
    path: &str,
    state: NotificationState,
    message: &str,
) {
    let notif = Notification {
        state,
        message: message.to_string(),
        method: vec![NotificationMethod::Visual],
        status: None,
    };
    let delta = Delta::self_vessel(vec![Update::new(
        Source::plugin("autopilot"),
        vec![PathValue::new(
            format!("notifications.{path}"),
            serde_json::to_value(notif).unwrap_or_default(),
        )],
    )]);
    let _ = ctx.handle_message(delta).await;
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_plugin_api::testing::MockPluginContext;

    #[test]
    fn plugin_metadata_correct() {
        let p = AutopilotPlugin::new();
        let m = p.metadata();
        assert_eq!(m.id, "autopilot");
        assert_eq!(m.name, "Autopilot");
    }

    #[test]
    fn default_config_has_d_term() {
        let cfg = AutopilotConfig::default();
        assert!(cfg.gain_d > 0.0);
        assert_eq!(cfg.gain_p, 1.0);
        assert_eq!(cfg.gain_d, 0.3);
    }

    #[test]
    fn default_config_has_xte_gain() {
        let cfg = AutopilotConfig::default();
        assert!(cfg.xte_gain > 0.0);
        assert_eq!(cfg.xte_gain, 0.01);
    }

    #[test]
    fn default_config_has_wind_filter() {
        let cfg = AutopilotConfig::default();
        assert!(cfg.wind_filter_alpha > 0.0 && cfg.wind_filter_alpha <= 1.0);
    }

    #[tokio::test]
    async fn plugin_start_succeeds() {
        let mut plugin = AutopilotPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());
        plugin
            .start(serde_json::json!({}), ctx.clone())
            .await
            .unwrap();
        // Provider was registered (start returned Ok) and status was set
        let msgs = ctx.status_messages.lock().unwrap();
        assert!(msgs.iter().any(|s| s == "Standby"));
    }
}
