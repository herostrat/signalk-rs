/// AIS target tracking plugin for signalk-rs.
///
/// Subscribes to ALL vessel deltas, tracks AIS targets through a lifecycle
/// state machine (Acquiring → Tracking → Lost → Removed), and emits
/// `sensors.ais.status` and `sensors.ais.class` deltas for each target.
///
/// Config example (signalk-rs.toml):
/// ```toml
/// [[plugins]]
/// id = "ais-status"
/// enabled = true
/// config = { tick_interval_secs = 30 }
/// ```
pub mod cpa;
pub mod tracker;

use async_trait::async_trait;
use serde::Deserialize;
use signalk_plugin_api::{
    Plugin, PluginContext, PluginError, PluginMetadata, RouterSetup, SubscriptionHandle,
    SubscriptionSpec, delta_callback, route_handler,
};
use signalk_types::{Delta, PathValue, Source, Subscription, Update};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

use crate::tracker::{
    AisTracker, StatusTransition, TargetClass, TargetStatus, ThresholdOverrides, TrackerConfig,
};

// ─── Config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct AisConfig {
    /// Tick interval in seconds for checking lost/stale targets. Default: 30.
    #[serde(default = "default_tick_interval")]
    tick_interval_secs: u64,

    /// Per-class threshold overrides.
    #[serde(default)]
    thresholds: ThresholdsConfig,

    /// Issue a `warn` notification when CPA < this distance (nautical miles).
    /// `None` = CPA warnings disabled.
    #[serde(default)]
    cpa_warn_distance_nm: Option<f64>,

    /// Issue an `alarm` notification when CPA < this distance (nautical miles).
    /// `None` = CPA alarms disabled.
    #[serde(default)]
    cpa_alarm_distance_nm: Option<f64>,

    /// Ignore targets whose TCPA exceeds this threshold (seconds). Default: 3600.
    #[serde(default = "default_tcpa_max_secs")]
    tcpa_max_secs: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ThresholdsConfig {
    #[serde(default)]
    vessel: Option<ThresholdOverride>,
    #[serde(default)]
    aton: Option<ThresholdOverride>,
    #[serde(default)]
    base: Option<ThresholdOverride>,
    #[serde(default)]
    sar: Option<ThresholdOverride>,
}

/// Optional overrides for class thresholds. Unset fields use defaults.
#[derive(Debug, Clone, Deserialize)]
struct ThresholdOverride {
    confirm_count: Option<u32>,
    confirm_window_secs: Option<u64>,
    lost_after_secs: Option<u64>,
    remove_after_secs: Option<u64>,
}

impl From<&ThresholdOverride> for ThresholdOverrides {
    fn from(o: &ThresholdOverride) -> Self {
        ThresholdOverrides {
            confirm_count: o.confirm_count,
            confirm_window_secs: o.confirm_window_secs,
            lost_after_secs: o.lost_after_secs,
            remove_after_secs: o.remove_after_secs,
        }
    }
}

fn default_tick_interval() -> u64 {
    30
}

fn default_tcpa_max_secs() -> u64 {
    3600
}

impl Default for AisConfig {
    fn default() -> Self {
        AisConfig {
            tick_interval_secs: default_tick_interval(),
            thresholds: ThresholdsConfig::default(),
            cpa_warn_distance_nm: None,
            cpa_alarm_distance_nm: None,
            tcpa_max_secs: default_tcpa_max_secs(),
        }
    }
}

// ─── Plugin ─────────────────────────────────────────────────────────────────

pub struct AisStatusPlugin {
    subscription_handle: Option<SubscriptionHandle>,
    ctx: Option<Arc<dyn PluginContext>>,
    tick_abort: Option<tokio::sync::watch::Sender<bool>>,
}

impl AisStatusPlugin {
    pub fn new() -> Self {
        AisStatusPlugin {
            subscription_handle: None,
            ctx: None,
            tick_abort: None,
        }
    }
}

impl Default for AisStatusPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for AisStatusPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "ais-status",
            "AIS Target Status",
            "Tracks AIS targets — data fusion, lifecycle management, status reporting",
            env!("CARGO_PKG_VERSION"),
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        let threshold_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "confirm_count": { "type": "integer", "description": "Messages needed to confirm" },
                "confirm_window_secs": { "type": "integer", "description": "Window for confirm_count messages" },
                "lost_after_secs": { "type": "integer", "description": "Seconds without updates before Lost" },
                "remove_after_secs": { "type": "integer", "description": "Seconds in Lost before removal" }
            }
        });
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "tick_interval_secs": {
                    "type": "integer",
                    "description": "Tick interval in seconds for checking lost/stale targets",
                    "default": 30
                },
                "thresholds": {
                    "type": "object",
                    "description": "Per-class lifecycle thresholds (vessel, aton, base, sar)",
                    "properties": {
                        "vessel": threshold_schema,
                        "aton": threshold_schema,
                        "base": threshold_schema,
                        "sar": threshold_schema
                    }
                }
            }
        }))
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let ais_config: AisConfig = if config.is_null() || config == serde_json::json!({}) {
            AisConfig::default()
        } else {
            serde_json::from_value(config)
                .map_err(|e| PluginError::config(format!("invalid AIS config: {e}")))?
        };

        // Determine self vessel URI to ignore own vessel.
        // The tracker uses this to skip deltas from our own vessel.
        // "self" works because update_target checks context.ends_with(self_uri).
        let self_uri = "self".to_string();

        info!(
            tick_interval = ais_config.tick_interval_secs,
            "AIS Status plugin starting"
        );

        let tracker_config = TrackerConfig {
            vessel: ais_config.thresholds.vessel.as_ref().map(Into::into),
            aton: ais_config.thresholds.aton.as_ref().map(Into::into),
            base: ais_config.thresholds.base.as_ref().map(Into::into),
            sar: ais_config.thresholds.sar.as_ref().map(Into::into),
        };
        let tracker = Arc::new(Mutex::new(AisTracker::with_config(
            self_uri,
            tracker_config,
        )));

        // ── Register plugin routes (/plugins/ais-status/) ─────────────────────
        let tracker_api = tracker.clone();
        ctx.register_routes(
            Box::new(move |router: &mut dyn signalk_plugin_api::PluginRouter| {
                let t = tracker_api.clone();
                router.get(
                    "/targets",
                    route_handler(move |_req| {
                        let t = t.clone();
                        async move {
                            let snapshot = t.lock().unwrap().targets_snapshot();
                            signalk_plugin_api::PluginResponse::json(
                                200,
                                &serde_json::json!(snapshot),
                            )
                        }
                    }),
                );
            }) as RouterSetup,
        )
        .await?;

        // Subscribe to ALL vessel deltas
        let tracker_sub = tracker.clone();
        let ctx_sub = ctx.clone();

        let handle = ctx
            .subscribe(
                SubscriptionSpec::all_vessels(vec![Subscription::path("**")]),
                delta_callback(move |delta: Delta| {
                    let context = match delta.context.as_deref() {
                        Some(c) => c,
                        None => return,
                    };

                    // Only process vessel MMSI contexts
                    if !context.starts_with("vessels.urn:mrn:imo:mmsi:") {
                        return;
                    }

                    // Collect all path/value pairs from this delta
                    let values: Vec<(String, serde_json::Value)> = delta
                        .updates
                        .iter()
                        .flat_map(|u| {
                            u.values
                                .iter()
                                .map(|pv| (pv.path.clone(), pv.value.clone()))
                        })
                        .collect();

                    let transition = {
                        let mut tracker = tracker_sub.lock().unwrap();
                        tracker.update_target(context, &values, std::time::Instant::now())
                    };

                    if let Some(transition) = transition {
                        emit_status_delta(&ctx_sub, &transition);
                    }
                }),
            )
            .await?;

        // Start tick task for lost/remove detection + CPA computation
        let (abort_tx, mut abort_rx) = tokio::sync::watch::channel(false);
        let tracker_tick = tracker.clone();
        let ctx_tick = ctx.clone();
        let tick_interval = ais_config.tick_interval_secs;
        // Clone CPA config for the async task
        let cpa_warn_m = ais_config.cpa_warn_distance_nm.map(|nm| nm * 1852.0);
        let cpa_alarm_m = ais_config.cpa_alarm_distance_nm.map(|nm| nm * 1852.0);
        let tcpa_max_s = ais_config.tcpa_max_secs as f64;

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(tick_interval));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // ── 1. Lost/stale detection ──────────────────────────
                        let transitions = {
                            let mut tracker = tracker_tick.lock().unwrap();
                            let transitions = tracker.tick(std::time::Instant::now());
                            let (confirmed, lost, unconfirmed) = tracker.count_by_status();
                            ctx_tick.set_status(&format!(
                                "Tracking {} targets ({confirmed} confirmed, {lost} lost, {unconfirmed} unconfirmed)",
                                tracker.target_count()
                            ));
                            transitions
                        };

                        for transition in transitions {
                            emit_status_delta(&ctx_tick, &transition);
                        }

                        // ── 2. CPA computation (only when thresholds configured) ──
                        if cpa_warn_m.is_some() || cpa_alarm_m.is_some() {
                            run_cpa_tick(
                                &tracker_tick,
                                &ctx_tick,
                                cpa_warn_m,
                                cpa_alarm_m,
                                tcpa_max_s,
                            )
                            .await;
                        }
                    }
                    _ = abort_rx.changed() => {
                        if *abort_rx.borrow() {
                            debug!("AIS tick task stopping");
                            break;
                        }
                    }
                }
            }
        });

        self.subscription_handle = Some(handle);
        self.ctx = Some(ctx);
        self.tick_abort = Some(abort_tx);

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        // Stop tick task
        if let Some(abort_tx) = self.tick_abort.take() {
            let _ = abort_tx.send(true);
        }

        // Unsubscribe
        if let (Some(handle), Some(ctx)) = (self.subscription_handle.take(), self.ctx.take()) {
            ctx.unsubscribe(handle).await?;
        }

        info!("AIS Status plugin stopped");
        Ok(())
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Emit a status delta for a target transition.
fn emit_status_delta(ctx: &Arc<dyn PluginContext>, transition: &StatusTransition) {
    let mut values = vec![PathValue::new(
        "sensors.ais.status",
        serde_json::json!(transition.new_status.as_str()),
    )];

    // Also emit class on first tracking
    if transition.old_status == TargetStatus::Acquiring
        && transition.new_status == TargetStatus::Tracking
    {
        let class = tracker::classify_mmsi(
            AisTracker::parse_mmsi(&transition.context).unwrap_or(&transition.mmsi),
        );
        values.push(PathValue::new(
            "sensors.ais.class",
            serde_json::json!(class_to_str(class)),
        ));
    }

    let delta = Delta::with_context(
        transition.context.clone(),
        vec![Update::new(Source::plugin("ais-status"), values)],
    );

    let ctx = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = ctx.handle_message(delta).await {
            warn!("Failed to emit AIS status delta: {e}");
        }
    });
}

fn class_to_str(class: TargetClass) -> &'static str {
    match class {
        TargetClass::Vessel => "A",
        TargetClass::Aton => "aton",
        TargetClass::Base => "base",
        TargetClass::Sar => "sar",
    }
}

// ─── CPA helpers ─────────────────────────────────────────────────────────────

/// Pure alarm classification — no side effects, easily testable.
///
/// Returns `Some("alarm")`, `Some("warn")`, or `None` based on CPA/TCPA values
/// and the configured thresholds. Does NOT consider previous alarm state.
pub(crate) fn classify_cpa_alarm(
    cpa_m: f64,
    tcpa_s: f64,
    tcpa_max_s: f64,
    cpa_warn_m: Option<f64>,
    cpa_alarm_m: Option<f64>,
) -> Option<&'static str> {
    let is_future_threat = tcpa_s > 0.0 && tcpa_s < tcpa_max_s;
    if is_future_threat {
        if cpa_alarm_m.is_some_and(|d| cpa_m < d) {
            Some("alarm")
        } else if cpa_warn_m.is_some_and(|d| cpa_m < d) {
            Some("warn")
        } else {
            None
        }
    } else {
        None
    }
}

/// One CPA tick: read own vessel data, compute CPA for all confirmed targets,
/// emit deltas, and manage collision notifications on state transitions.
async fn run_cpa_tick(
    tracker: &std::sync::Mutex<AisTracker>,
    ctx: &Arc<dyn PluginContext>,
    cpa_warn_m: Option<f64>,
    cpa_alarm_m: Option<f64>,
    tcpa_max_s: f64,
) {
    // Fetch own position + motion from store
    let own_pos = match ctx.get_self_path("navigation.position").await {
        Ok(Some(v)) => v,
        _ => return,
    };
    let own_lat = match own_pos.get("latitude").and_then(|v| v.as_f64()) {
        Some(v) => v,
        None => return,
    };
    let own_lon = match own_pos.get("longitude").and_then(|v| v.as_f64()) {
        Some(v) => v,
        None => return,
    };
    let own_sog_ms = ctx
        .get_self_path("navigation.speedOverGround")
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let own_cog_rad = ctx
        .get_self_path("navigation.courseOverGroundTrue")
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    // Snapshot confirmed targets (no lock held for async calls below)
    let snapshots = {
        let trk = tracker.lock().unwrap();
        trk.targets_for_cpa()
    };

    // Event: a target that needs an alarm state change
    struct CpaAlarmChange {
        mmsi: String,
        alarm_level: Option<&'static str>, // Some = raise, None = clear
    }

    let mut cpa_deltas: Vec<(String, f64, f64)> = Vec::new(); // (context, cpa_m, tcpa_s)
    let mut alarm_changes: Vec<CpaAlarmChange> = Vec::new();

    for snap in &snapshots {
        let Some(result) = cpa::compute_cpa(
            own_lat,
            own_lon,
            own_sog_ms,
            own_cog_rad,
            snap.lat,
            snap.lon,
            snap.sog_ms,
            snap.cog_rad,
        ) else {
            continue;
        };

        cpa_deltas.push((snap.context.clone(), result.cpa_m, result.tcpa_s));

        // Determine alarm level
        let alarm_level = classify_cpa_alarm(
            result.cpa_m,
            result.tcpa_s,
            tcpa_max_s,
            cpa_warn_m,
            cpa_alarm_m,
        );

        let is_active = alarm_level.is_some();
        if is_active != snap.cpa_alarm_active {
            // Update tracker state (synchronous, brief lock)
            tracker.lock().unwrap().update_target_cpa(
                &snap.mmsi,
                result.cpa_m,
                result.tcpa_s,
                is_active,
            );

            alarm_changes.push(CpaAlarmChange {
                mmsi: snap.mmsi.clone(),
                alarm_level,
            });
        } else {
            // No alarm change, still update CPA data
            tracker.lock().unwrap().update_target_cpa(
                &snap.mmsi,
                result.cpa_m,
                result.tcpa_s,
                snap.cpa_alarm_active,
            );
        }
    }

    // Emit CPA data deltas (target vessel context)
    for (context, cpa_m, tcpa_s) in cpa_deltas {
        let delta = Delta::with_context(
            context,
            vec![Update::new(
                Source::plugin("ais-status"),
                vec![
                    PathValue::new(
                        "navigation.closestApproach.distance",
                        serde_json::json!(cpa_m),
                    ),
                    PathValue::new(
                        "navigation.closestApproach.timeTo",
                        serde_json::json!(tcpa_s),
                    ),
                ],
            )],
        );
        if let Err(e) = ctx.handle_message(delta).await {
            warn!("Failed to emit CPA delta: {e}");
        }
    }

    // Emit alarm state changes
    for change in alarm_changes {
        if let Some(level) = change.alarm_level {
            let method = if level == "alarm" {
                serde_json::json!(["visual", "sound"])
            } else {
                serde_json::json!(["visual"])
            };
            let notification = serde_json::json!({
                "state": level,
                "method": method,
                "message": format!("Collision risk: MMSI {}", change.mmsi)
            });
            let notif_path = format!("collision.mmsi-{}", change.mmsi);
            if let Err(e) = ctx
                .handle_message(Delta::self_vessel(vec![Update::new(
                    Source::plugin("ais-status"),
                    vec![PathValue::new(
                        format!("notifications.{notif_path}"),
                        notification,
                    )],
                )]))
                .await
            {
                warn!("Failed to raise CPA alarm: {e}");
            }
        } else {
            // Clear: set to normal
            if let Err(e) = ctx
                .handle_message(Delta::self_vessel(vec![Update::new(
                    Source::plugin("ais-status"),
                    vec![PathValue::new(
                        format!("notifications.collision.mmsi-{}", change.mmsi),
                        serde_json::json!({"state": "normal", "method": [], "message": ""}),
                    )],
                )]))
                .await
            {
                warn!("Failed to clear CPA alarm: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_cpa_alarm ───────────────────────────────────────────

    const WARN_M: f64 = 1000.0; // 1 km warn distance
    const ALARM_M: f64 = 500.0; // 500 m alarm distance
    const MAX_S: f64 = 3600.0; // ignore threats > 1 hour away

    #[test]
    fn alarm_when_cpa_below_alarm_distance() {
        // CPA 300 m, TCPA 5 min → alarm level
        let level = classify_cpa_alarm(300.0, 300.0, MAX_S, Some(WARN_M), Some(ALARM_M));
        assert_eq!(level, Some("alarm"));
    }

    #[test]
    fn warn_when_cpa_between_warn_and_alarm_distance() {
        // CPA 800 m (< warn 1000, > alarm 500) → warn level
        let level = classify_cpa_alarm(800.0, 300.0, MAX_S, Some(WARN_M), Some(ALARM_M));
        assert_eq!(level, Some("warn"));
    }

    #[test]
    fn none_when_cpa_outside_warn_distance() {
        // CPA 2 km — safe
        let level = classify_cpa_alarm(2000.0, 300.0, MAX_S, Some(WARN_M), Some(ALARM_M));
        assert_eq!(level, None);
    }

    #[test]
    fn none_when_tcpa_is_negative_past_cpa() {
        // TCPA < 0 means closest approach already happened — no threat
        let level = classify_cpa_alarm(100.0, -60.0, MAX_S, Some(WARN_M), Some(ALARM_M));
        assert_eq!(level, None);
    }

    #[test]
    fn none_when_tcpa_exceeds_max() {
        // TCPA 2 hours, max 1 hour → too far away to care
        let level = classify_cpa_alarm(100.0, 7200.0, MAX_S, Some(WARN_M), Some(ALARM_M));
        assert_eq!(level, None);
    }

    #[test]
    fn none_when_thresholds_disabled() {
        // Both thresholds None → CPA alarm system disabled
        let level = classify_cpa_alarm(10.0, 30.0, MAX_S, None, None);
        assert_eq!(level, None);
    }

    #[test]
    fn warn_only_config_gives_no_alarm_level() {
        // Only warn configured (no alarm threshold) → can only produce "warn", never "alarm"
        let level = classify_cpa_alarm(100.0, 300.0, MAX_S, Some(WARN_M), None);
        assert_eq!(level, Some("warn"));
    }

    #[test]
    fn alarm_only_config_does_not_produce_warn() {
        // Only alarm threshold configured, CPA between alarm and would-be-warn
        // cpa_m 800 > alarm_m 500 → no alarm; no warn threshold → None
        let level = classify_cpa_alarm(800.0, 300.0, MAX_S, None, Some(ALARM_M));
        assert_eq!(level, None);
    }

    #[test]
    fn tcpa_zero_is_not_a_future_threat() {
        // TCPA = 0 means passing right now — treated as past, no alarm
        let level = classify_cpa_alarm(10.0, 0.0, MAX_S, Some(WARN_M), Some(ALARM_M));
        assert_eq!(level, None);
    }
}
