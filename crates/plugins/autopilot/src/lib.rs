//! Software autopilot plugin for signalk-rs.
//!
//! Registers as a SignalK V2 `AutopilotProvider` and implements a PID heading
//! controller with gain scheduling, heel compensation, gust response, recovery
//! mode, and rudder rate limiting.
//!
//! # Data flow
//! ```text
//! SK navigation.headingMagnetic ──► [subscription callback]
//! SK environment.wind.angle*                               │
//! SK environment.wind.speedApparent                        │
//! SK navigation.rateOfTurn                                 │
//! SK navigation.speedThroughWater                          │
//! SK navigation.attitude.roll                              ▼
//!                                            AutopilotState (Arc<RwLock<>>)
//!                                                         │
//!                                            [control loop, 10 Hz]
//!                                                         │
//!                  ┌──────────────────────────────────────-┤
//!              Compass          Wind / WindTrue       Route (cascaded)
//!         heading::compute()    CircularFilter +    route::compute()
//!         (PID + yaw_rate)        PID controller   outer: XTE→heading
//!                  └──────────────────────────────────────-┤
//!                                                         │
//!                                            ┌────────────┴────────────┐
//!                                            │ + recovery mode (2×P,D)│
//!                                            │ + heel feedforward     │
//!                                            │ + gust feedforward     │
//!                                            │ + gain scheduling      │
//!                                            │ + rate limiting        │
//!                                            └────────────┬────────────┘
//!                                                         │
//!                                                         ▼
//!                                    SK steering.rudderAngle  ──► hardware driver
//!                                    SK steering.autopilot.{state,mode}
//! ```
pub mod filter;
pub mod modes;
pub mod pd;
pub(crate) mod provider;
pub mod state;

use async_trait::async_trait;
use filter::{CircularFilter, RateDetector};
use pd::{
    HeadingPlausibility, PidController, PlausibilityResult, RecoveryState, RudderFeedbackMonitor,
};
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
            "Software autopilot — PID controller, compass/wind/route modes",
            "0.3.0",
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::to_value(schemars::schema_for!(AutopilotConfig)).unwrap())
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
                    Subscription::path("navigation.speedThroughWater"),
                    Subscription::path("navigation.speedOverGround"),
                    Subscription::path("navigation.attitude.roll"),
                    Subscription::path("environment.wind.angleApparent"),
                    Subscription::path("environment.wind.angleTrue"),
                    Subscription::path("environment.wind.speedApparent"),
                    Subscription::path("navigation.course.calcValues.bearingTrackTrue"),
                    Subscription::path("navigation.crossTrackError"),
                    Subscription::path("steering.rudderAngle"),
                ]),
                delta_callback(move |delta: Delta| {
                    let state = Arc::clone(&state_cb);
                    tokio::spawn(async move {
                        let mut st = state.write().await;
                        for update in &delta.updates {
                            let is_autopilot_source = update.source.type_ == "Plugin"
                                && update.source.label == "autopilot";
                            for pv in &update.values {
                                if let Some(v) = pv.value.as_f64() {
                                    // Rudder feedback: only store from hardware sensors
                                    if pv.path == "steering.rudderAngle" {
                                        if !is_autopilot_source {
                                            st.actual_rudder_rad = Some(v);
                                            st.actual_rudder_last_seen =
                                                Some(std::time::Instant::now());
                                        }
                                        continue;
                                    }
                                    st.update_sensor(&pv.path, v);
                                }
                            }
                        }
                    });
                }),
            )
            .await?;
        self.subscription_handle = Some(handle);

        // ── Snapshot helper (avoids 15-element tuple) ───────────────────────────
        struct Snapshot {
            enabled: bool,
            mode: AutopilotMode,
            target: Option<f64>,
            dodge: Option<f64>,
            primary_sensor: Option<f64>,
            timed_out: bool,
            prev_error: f64,
            last_tick: Option<Instant>,
            last_rudder: f64,
            heading: Option<f64>,
            xte: Option<f64>,
            yaw_rate: Option<f64>,
            speed: Option<f64>,
            heel: Option<f64>,
            wind_speed: Option<f64>,
            actual_rudder: Option<f64>,
            heading_age_secs: f64,
        }

        // ── Control loop ───────────────────────────────────────────────────────
        let state_tick = Arc::clone(&self.state);
        let ctx_tick = Arc::clone(&ctx);
        let cfg = self.config.clone();
        let control_interval =
            std::time::Duration::from_secs_f64(1.0 / cfg.control_rate_hz.max(0.1));
        let output_every_n = (cfg.control_rate_hz / cfg.output_rate_hz.max(0.1))
            .round()
            .max(1.0) as u64;

        let tick_abort = tokio::spawn(async move {
            // PID controller — maintains integral state across ticks
            let mut pid = PidController::new(cfg.integral_limit);
            // Circular low-pass filter for wind angle — wrap-safe at ±π
            let mut wind_filter = CircularFilter::new(cfg.wind_filter_alpha);
            // Recovery mode — temporary aggressive gains for large deviations
            let mut recovery = RecoveryState::new();
            // Gust detector — d(AWS)/dt for wind speed feedforward
            let mut gust_detector = RateDetector::new(0.5);
            // Rudder feedback monitor — detects hardware steering failure
            let mut rudder_feedback = RudderFeedbackMonitor::new();
            // Heading plausibility — detects sensor spikes / EMI glitches
            let mut heading_plausibility = HeadingPlausibility::new(cfg.heading_glitch_max_count);
            let mut prev_mode: Option<AutopilotMode> = None;
            let mut tick_counter: u64 = 0;

            loop {
                tokio::time::sleep(control_interval).await;
                tick_counter += 1;
                let should_emit = tick_counter.is_multiple_of(output_every_n);

                // ── Read snapshot from shared state ────────────────────────────
                let snapshot = {
                    let st = state_tick.read().await;
                    let heading = st
                        .sensor_values
                        .get("navigation.headingMagnetic")
                        .or_else(|| st.sensor_values.get("navigation.headingTrue"))
                        .copied();
                    let speed = st
                        .sensor_values
                        .get("navigation.speedThroughWater")
                        .or_else(|| st.sensor_values.get("navigation.speedOverGround"))
                        .copied();

                    let heading_age_secs = st
                        .sensor_last_seen
                        .get("navigation.headingMagnetic")
                        .or_else(|| st.sensor_last_seen.get("navigation.headingTrue"))
                        .map(|t| t.elapsed().as_secs_f64())
                        .unwrap_or(f64::INFINITY);

                    Snapshot {
                        enabled: st.enabled,
                        mode: st.mode.clone(),
                        target: st.target_rad,
                        dodge: st.dodge_offset_rad,
                        primary_sensor: st.current_sensor(),
                        timed_out: st.sensor_timed_out(cfg.sensor_timeout_secs),
                        prev_error: st.last_error_rad,
                        last_tick: st.last_tick_at,
                        last_rudder: st.last_rudder_rad,
                        heading,
                        xte: st.sensor_values.get("navigation.crossTrackError").copied(),
                        yaw_rate: st.sensor_values.get("navigation.rateOfTurn").copied(),
                        speed,
                        heel: st.sensor_values.get("navigation.attitude.roll").copied(),
                        wind_speed: st
                            .sensor_values
                            .get("environment.wind.speedApparent")
                            .copied(),
                        actual_rudder: st.actual_rudder_rad,
                        heading_age_secs,
                    }
                };
                let Snapshot {
                    enabled,
                    mode,
                    target,
                    dodge,
                    primary_sensor,
                    timed_out,
                    prev_error,
                    last_tick,
                    last_rudder,
                    heading,
                    xte,
                    yaw_rate,
                    speed,
                    heel,
                    wind_speed,
                    actual_rudder,
                    heading_age_secs,
                } = snapshot;

                // ── Emit autopilot state at output rate ──────────────────────
                if should_emit {
                    emit_autopilot_state(
                        &ctx_tick,
                        if enabled { "enabled" } else { "disabled" },
                        mode.as_str(),
                        target,
                        &mode,
                    )
                    .await;
                }

                if !enabled {
                    continue;
                }

                // ── Detect mode change → reset PID + filters ─────────────────
                if prev_mode.as_ref() != Some(&mode) {
                    pid.reset();
                    wind_filter.reset();
                    recovery.reset();
                    gust_detector.reset();
                    rudder_feedback.reset();
                    heading_plausibility.reset();
                    prev_mode = Some(mode.clone());
                }

                // ── Sensor timeout → alarm and disengage ─────────────────────
                if timed_out {
                    warn!(mode = %mode.as_str(), "Autopilot: sensor timeout");
                    {
                        let mut st = state_tick.write().await;
                        st.enabled = false;
                        st.last_rudder_rad = 0.0;
                        st.last_tick_at = None;
                    }
                    pid.reset();
                    recovery.reset();
                    rudder_feedback.reset();
                    heading_plausibility.reset();
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
                    None => continue,
                };
                let effective_target = target_rad + dodge.unwrap_or(0.0);

                // ── Compute dt from last tick ────────────────────────────────
                let now = Instant::now();
                let dt = match last_tick {
                    Some(t) => now.duration_since(t).as_secs_f64().max(0.001),
                    None => 1.0 / cfg.control_rate_hz,
                };

                // ── Heading plausibility check ────────────────────────────────
                // Reject sensor spikes (EMI, compass glitch). Single glitch →
                // discard + use prev heading. N consecutive → disengage.
                let heading = if let Some(h) = heading {
                    match heading_plausibility.check(h, cfg.max_heading_rate_rad_per_sec, dt) {
                        PlausibilityResult::Ok(v) => Some(v),
                        PlausibilityResult::Glitch(prev) => {
                            warn!(
                                raw = %h.to_degrees(),
                                prev = %prev.to_degrees(),
                                "Autopilot: heading glitch discarded"
                            );
                            Some(prev)
                        }
                        PlausibilityResult::SensorFailure => {
                            warn!("Autopilot: heading sensor failure — disengaging");
                            {
                                let mut st = state_tick.write().await;
                                st.enabled = false;
                                st.last_rudder_rad = 0.0;
                                st.last_tick_at = None;
                            }
                            pid.reset();
                            recovery.reset();
                            rudder_feedback.reset();
                            heading_plausibility.reset();
                            emit_rudder(&ctx_tick, 0.0).await;
                            emit_notification(
                                &ctx_tick,
                                "steering.autopilot.heading",
                                NotificationState::Alarm,
                                "Heading sensor glitch — autopilot disengaged",
                            )
                            .await;
                            ctx_tick.set_status("Heading sensor failure — disengaged");
                            continue;
                        }
                    }
                } else {
                    None
                };

                // ── Yaw-rate validation ───────────────────────────────────────
                // Reject implausible ROT readings → fallback to finite-diff.
                let yaw_rate = pd::validate_yaw_rate(yaw_rate, cfg.max_yaw_rate_rad_per_sec);

                // ── Speed-dependent gain scheduling ──────────────────────────
                let base_pid_cfg = cfg.pid_config();
                let pid_cfg = match speed {
                    Some(s) => pd::scale_gains(&base_pid_cfg, s, cfg.speed_nominal_mps),
                    None => base_pid_cfg,
                };

                // ── D-term quality scaling ────────────────────────────────────
                // Attenuate D-gain when heading sensor data is stale.
                let pid_cfg = if cfg.dterm_quality_half_life_secs > 0.0 {
                    let quality =
                        pd::sensor_quality(heading_age_secs, cfg.dterm_quality_half_life_secs);
                    pd::PidConfig {
                        gain_d: pid_cfg.gain_d * quality,
                        ..pid_cfg
                    }
                } else {
                    pid_cfg
                };

                // ── Recovery mode check ─────────────────────────────────────
                // (must happen before mode dispatch so recovery can modify gains)
                let pre_recovery_error = match mode {
                    AutopilotMode::Compass => {
                        heading.map(|h| pd::normalize_angle(effective_target - h))
                    }
                    _ => primary_sensor.map(|s| pd::normalize_angle(effective_target - s)),
                };
                if let Some(err) = pre_recovery_error {
                    recovery.update(err, cfg.recovery_threshold_rad, cfg.recovery_max_ticks);
                }
                let active_pid_cfg = recovery.apply(&pid_cfg, cfg.recovery_gain_factor);

                // ── Mode-specific control dispatch ───────────────────────────
                let (raw_rudder, new_error) = match mode {
                    // ── Compass: heading hold with PID + yaw-rate D-term ─────
                    AutopilotMode::Compass => {
                        let current = match heading {
                            Some(h) => h,
                            None => continue,
                        };
                        modes::heading::compute(
                            current,
                            effective_target,
                            prev_error,
                            dt,
                            yaw_rate,
                            &mut pid,
                            &active_pid_cfg,
                        )
                    }

                    // ── Wind (AWA): circular-filtered hold with PID ──────────
                    AutopilotMode::Wind => {
                        let raw = match primary_sensor {
                            Some(v) => v,
                            None => continue,
                        };
                        let current = wind_filter.update(raw);
                        let error = pd::normalize_angle(effective_target - current);
                        let d_error = match yaw_rate {
                            Some(rate) => -rate,
                            None => pd::normalize_angle(error - prev_error) / dt,
                        };
                        let rudder = pid.compute(error, d_error, dt, &active_pid_cfg);
                        (rudder, error)
                    }

                    // ── Wind True (TWA): same as AWA but reads angleTrue ─────
                    #[cfg(feature = "experimental")]
                    AutopilotMode::WindTrue => {
                        let raw = match primary_sensor {
                            Some(v) => v,
                            None => continue,
                        };
                        let current = wind_filter.update(raw);
                        let error = pd::normalize_angle(effective_target - current);
                        let d_error = match yaw_rate {
                            Some(rate) => -rate,
                            None => pd::normalize_angle(error - prev_error) / dt,
                        };
                        let rudder = pid.compute(error, d_error, dt, &active_pid_cfg);
                        (rudder, error)
                    }

                    // ── Route: cascaded LOS guidance ─────────────────────────
                    AutopilotMode::Route => {
                        let current_heading = match heading {
                            Some(h) => h,
                            None => continue,
                        };
                        let btw = match primary_sensor {
                            Some(v) => v,
                            None => continue,
                        };
                        let xte_m = xte.unwrap_or(0.0);
                        modes::route::compute(
                            &modes::route::RouteInput {
                                current_heading,
                                btw,
                                xte_m,
                                lookahead_m: cfg.route_lookahead_m,
                                prev_error,
                                dt,
                                yaw_rate,
                            },
                            &mut pid,
                            &active_pid_cfg,
                        )
                    }
                };

                // ── Heel compensation (feedforward) ──────────────────────────
                let heel_compensation = match heel {
                    Some(roll) if cfg.heel_gain != 0.0 => cfg.heel_gain * roll,
                    _ => 0.0,
                };

                // ── Gust response (wind speed feedforward) ─────────────────
                let gust_compensation = match wind_speed {
                    Some(ws) if cfg.gust_gain != 0.0 => {
                        let rate = gust_detector.update(ws, dt);
                        if rate.abs() > cfg.gust_threshold_mps_per_sec {
                            cfg.gust_gain * rate
                        } else {
                            0.0
                        }
                    }
                    _ => 0.0,
                };

                let rudder_with_ff = (raw_rudder + heel_compensation + gust_compensation)
                    .clamp(-cfg.max_rudder_rad, cfg.max_rudder_rad);

                // ── Rudder rate limiting ─────────────────────────────────────
                let rudder = pd::rate_limit(
                    last_rudder,
                    rudder_with_ff,
                    cfg.max_rudder_rate_rad_per_sec,
                    dt,
                );

                {
                    let mut st = state_tick.write().await;
                    st.last_rudder_rad = rudder;
                    st.last_error_rad = new_error;
                    st.last_tick_at = Some(now);
                }

                if should_emit {
                    emit_rudder(&ctx_tick, rudder).await;
                }

                // ── Rudder feedback monitoring ─────────────────────────────
                // Compare commanded vs actual rudder angle from hardware sensor.
                // If no feedback sensor exists, actual_rudder is None → skip.
                match rudder_feedback.update(
                    rudder,
                    actual_rudder,
                    cfg.rudder_feedback_threshold_rad,
                    cfg.rudder_feedback_timeout_ticks,
                ) {
                    Some(true) => {
                        // Alarm just fired
                        let actual = actual_rudder.unwrap_or(0.0);
                        warn!(
                            commanded = %rudder,
                            actual = %actual,
                            mismatch_deg = %((rudder - actual).abs().to_degrees()),
                            "Autopilot: rudder feedback mismatch"
                        );
                        emit_notification(
                            &ctx_tick,
                            "steering.autopilot.rudderFeedbackFailure",
                            NotificationState::Alarm,
                            &format!(
                                "Rudder not responding: commanded {:.1}°, actual {:.1}°",
                                rudder.to_degrees(),
                                actual.to_degrees()
                            ),
                        )
                        .await;
                        ctx_tick.set_status("Rudder feedback failure — check steering");
                    }
                    Some(false) => {
                        // Alarm just cleared
                        emit_notification(
                            &ctx_tick,
                            "steering.autopilot.rudderFeedbackFailure",
                            NotificationState::Normal,
                            "Rudder feedback restored",
                        )
                        .await;
                        ctx_tick.set_status("Active");
                    }
                    None => {} // no change
                }
            }
        })
        .abort_handle();
        self.tick_handle = Some(tick_abort);

        // ── Register as autopilot provider ────────────────────────────────────
        let device_id = self.config.device_id.clone();
        let provider = Arc::new(ProviderHandle {
            device_id,
            state: Arc::clone(&self.state),
            ctx: Arc::clone(&ctx),
        });
        ctx.register_autopilot_provider(provider).await?;

        info!(
            device_id = %self.config.device_id,
            mode = %self.config.initial_mode,
            control_hz = %self.config.control_rate_hz,
            output_hz = %self.config.output_rate_hz,
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

pub(crate) async fn emit_autopilot_state(
    ctx: &Arc<dyn PluginContext>,
    state_str: &str,
    mode_str: &str,
    target_rad: Option<f64>,
    mode: &AutopilotMode,
) {
    let source = Source::plugin("autopilot");
    let engaged = state_str == "enabled";
    let is_wind_mode = *mode == AutopilotMode::Wind;
    #[cfg(feature = "experimental")]
    let is_wind_mode = is_wind_mode || *mode == AutopilotMode::WindTrue;

    let mut values = vec![
        PathValue::new("steering.autopilot.state", serde_json::json!(state_str)),
        PathValue::new("steering.autopilot.mode", serde_json::json!(mode_str)),
        PathValue::new("steering.autopilot.engaged", serde_json::json!(engaged)),
    ];
    if let Some(target) = target_rad {
        values.push(PathValue::new(
            mode.target_path(),
            serde_json::json!(target),
        ));
    }
    // Emit available actions (dodge/tack/gybe based on local state;
    // courseCurrentPoint/courseNextPoint depend on store, omitted here).
    values.push(PathValue::new(
        "steering.autopilot.actions",
        serde_json::json!({
            "dodge": engaged,
            "tack": engaged && is_wind_mode,
            "gybe": engaged && is_wind_mode,
        }),
    ));
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
        id: None,
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
    fn default_config_pid_gains() {
        let cfg = AutopilotConfig::default();
        assert_eq!(cfg.gain_p, 1.0);
        assert_eq!(cfg.gain_i, 0.05);
        assert_eq!(cfg.gain_d, 0.3);
        assert_eq!(cfg.integral_limit, 0.5);
    }

    #[test]
    fn default_config_gain_scheduling() {
        let cfg = AutopilotConfig::default();
        assert_eq!(cfg.speed_nominal_mps, 2.5);
    }

    #[test]
    fn default_config_heel_compensation() {
        let cfg = AutopilotConfig::default();
        assert_eq!(cfg.heel_gain, -0.5);
    }

    #[test]
    fn default_config_rate_limiting() {
        let cfg = AutopilotConfig::default();
        assert!((cfg.max_rudder_rate_rad_per_sec - 0.09).abs() < 1e-10);
    }

    #[test]
    fn default_config_tick_rates() {
        let cfg = AutopilotConfig::default();
        assert_eq!(cfg.control_rate_hz, 10.0);
        assert_eq!(cfg.output_rate_hz, 1.0);
    }

    #[test]
    fn default_config_recovery() {
        let cfg = AutopilotConfig::default();
        assert!((cfg.recovery_threshold_rad - 0.35).abs() < 1e-10);
        assert_eq!(cfg.recovery_max_ticks, 15);
        assert!((cfg.recovery_gain_factor - 2.0).abs() < 1e-10);
    }

    #[test]
    fn default_config_gust() {
        let cfg = AutopilotConfig::default();
        assert!((cfg.gust_gain - (-0.02)).abs() < 1e-10);
        assert!((cfg.gust_threshold_mps_per_sec - 3.0).abs() < 1e-10);
    }

    #[test]
    fn default_config_route_lookahead() {
        let cfg = AutopilotConfig::default();
        assert!((cfg.route_lookahead_m - 100.0).abs() < 1e-10);
    }

    #[test]
    fn default_config_rudder_feedback() {
        let cfg = AutopilotConfig::default();
        assert!((cfg.rudder_feedback_threshold_rad - 0.087).abs() < 1e-10);
        assert_eq!(cfg.rudder_feedback_timeout_ticks, 30);
    }

    #[test]
    fn default_config_heading_plausibility() {
        let cfg = AutopilotConfig::default();
        assert!((cfg.max_heading_rate_rad_per_sec - 1.5).abs() < 1e-10);
        assert!((cfg.max_yaw_rate_rad_per_sec - 0.8).abs() < 1e-10);
        assert_eq!(cfg.heading_glitch_max_count, 3);
        assert!((cfg.dterm_quality_half_life_secs - 0.5).abs() < 1e-10);
    }

    #[test]
    fn pid_config_from_autopilot_config() {
        let cfg = AutopilotConfig::default();
        let pid_cfg = cfg.pid_config();
        assert_eq!(pid_cfg.gain_p, cfg.gain_p);
        assert_eq!(pid_cfg.gain_i, cfg.gain_i);
        assert_eq!(pid_cfg.gain_d, cfg.gain_d);
    }

    #[tokio::test]
    async fn plugin_start_succeeds() {
        let mut plugin = AutopilotPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());
        plugin
            .start(serde_json::json!({}), ctx.clone())
            .await
            .unwrap();
        let msgs = ctx.status_messages.lock().unwrap();
        assert!(msgs.iter().any(|s| s == "Standby"));
    }
}
