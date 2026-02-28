/// AIS target tracking plugin for signalk-rs.
///
/// Subscribes to ALL vessel deltas, tracks AIS targets through a lifecycle
/// state machine (Unconfirmed → Confirmed → Lost → Removed), and emits
/// `sensors.ais.status` and `sensors.ais.class` deltas for each target.
///
/// Config example (signalk-rs.toml):
/// ```toml
/// [[plugins]]
/// id = "ais-status"
/// enabled = true
/// config = { tick_interval_secs = 30 }
/// ```
pub mod tracker;

use async_trait::async_trait;
use serde::Deserialize;
use signalk_plugin_api::{
    Plugin, PluginContext, PluginError, PluginMetadata, SubscriptionHandle, SubscriptionSpec,
    delta_callback,
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

impl Default for AisConfig {
    fn default() -> Self {
        AisConfig {
            tick_interval_secs: default_tick_interval(),
            thresholds: ThresholdsConfig::default(),
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

        // Start tick task for lost/remove detection
        let (abort_tx, mut abort_rx) = tokio::sync::watch::channel(false);
        let tracker_tick = tracker.clone();
        let ctx_tick = ctx.clone();
        let tick_interval = ais_config.tick_interval_secs;

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(tick_interval));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
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

    // Also emit class on first confirmation
    if transition.old_status == TargetStatus::Unconfirmed
        && transition.new_status == TargetStatus::Confirmed
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
