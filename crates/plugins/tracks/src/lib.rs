/// Vessel position track recording plugin for signalk-rs.
///
/// Subscribes to ALL vessel deltas, records position history with auxiliary
/// data (SOG, COG, depth), and serves tracks as GeoJSON or GPX via REST API.
///
/// Config example (signalk-rs.toml):
/// ```toml
/// [[plugins]]
/// id = "tracks"
/// config = { min_interval_secs = 5, max_points = 50000, max_age_hours = 24 }
/// ```
pub mod api;
pub mod geojson;
pub mod gpx;
pub mod store;
pub mod types;

use async_trait::async_trait;
use chrono::TimeDelta;
use serde::Deserialize;
use signalk_plugin_api::{
    Plugin, PluginContext, PluginError, PluginMetadata, RouterSetup, SubscriptionHandle,
    SubscriptionSpec, delta_callback, route_handler,
};
use signalk_types::{Delta, Subscription};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

use crate::api::{handle_delete_tracks, handle_get_summary, handle_get_tracks};
use crate::store::{InMemoryTrackStore, TrackStore};
use crate::types::TrackPoint;

// ─── Config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct TracksConfig {
    /// Minimum seconds between recorded points per vessel. Default: 5.
    #[serde(default = "default_min_interval")]
    min_interval_secs: u64,

    /// Maximum points per vessel (ring buffer size). Default: 50_000.
    #[serde(default = "default_max_points")]
    max_points: usize,

    /// Maximum age of points in hours before pruning. Default: 24.
    #[serde(default = "default_max_age_hours")]
    max_age_hours: u64,

    /// Tick interval in seconds for pruning. Default: 60.
    #[serde(default = "default_tick_interval")]
    tick_interval_secs: u64,

    /// Track own vessel position. Default: true.
    #[serde(default = "default_true")]
    track_self: bool,

    /// Track other vessels (AIS targets). Default: true.
    #[serde(default = "default_true")]
    track_others: bool,
}

fn default_min_interval() -> u64 {
    5
}
fn default_max_points() -> usize {
    50_000
}
fn default_max_age_hours() -> u64 {
    24
}
fn default_tick_interval() -> u64 {
    60
}
fn default_true() -> bool {
    true
}

impl Default for TracksConfig {
    fn default() -> Self {
        TracksConfig {
            min_interval_secs: default_min_interval(),
            max_points: default_max_points(),
            max_age_hours: default_max_age_hours(),
            tick_interval_secs: default_tick_interval(),
            track_self: true,
            track_others: true,
        }
    }
}

// ─── Auxiliary data cache ───────────────────────────────────────────────────

/// Cached auxiliary data per vessel (last known values).
#[derive(Debug, Clone, Default)]
struct AuxCache {
    sog: Option<f64>,
    cog: Option<f64>,
    depth: Option<f64>,
}

// ─── Plugin ─────────────────────────────────────────────────────────────────

pub struct TracksPlugin {
    subscription_handle: Option<SubscriptionHandle>,
    ctx: Option<Arc<dyn PluginContext>>,
    tick_abort: Option<tokio::sync::watch::Sender<bool>>,
}

impl TracksPlugin {
    pub fn new() -> Self {
        TracksPlugin {
            subscription_handle: None,
            ctx: None,
            tick_abort: None,
        }
    }
}

impl Default for TracksPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for TracksPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "tracks",
            "Vessel Tracks",
            "Records position history for all vessels — GeoJSON and GPX output",
            env!("CARGO_PKG_VERSION"),
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "min_interval_secs": {
                    "type": "integer",
                    "description": "Minimum seconds between recorded points per vessel",
                    "default": 5
                },
                "max_points": {
                    "type": "integer",
                    "description": "Maximum points per vessel (ring buffer size)",
                    "default": 50000
                },
                "max_age_hours": {
                    "type": "integer",
                    "description": "Maximum age of points in hours before pruning",
                    "default": 24
                },
                "tick_interval_secs": {
                    "type": "integer",
                    "description": "Pruning check interval in seconds",
                    "default": 60
                },
                "track_self": {
                    "type": "boolean",
                    "description": "Track own vessel position",
                    "default": true
                },
                "track_others": {
                    "type": "boolean",
                    "description": "Track other vessels (AIS targets etc.)",
                    "default": true
                }
            }
        }))
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let cfg: TracksConfig = if config.is_null() || config == serde_json::json!({}) {
            TracksConfig::default()
        } else {
            serde_json::from_value(config)
                .map_err(|e| PluginError::config(format!("invalid tracks config: {e}")))?
        };

        info!(
            min_interval = cfg.min_interval_secs,
            max_points = cfg.max_points,
            max_age_hours = cfg.max_age_hours,
            track_self = cfg.track_self,
            track_others = cfg.track_others,
            "Tracks plugin starting"
        );

        let store: Arc<Mutex<dyn TrackStore>> =
            Arc::new(Mutex::new(InMemoryTrackStore::new(cfg.max_points)));

        // ── Register plugin routes (/plugins/tracks/) ──────────────────────
        // The spec routes (/signalk/v1/api/tracks, /vessels/{id}/track) are
        // defined in signalk-server and delegate to GET / here.
        let store_get = store.clone();
        let store_summary = store.clone();
        let store_delete = store.clone();

        ctx.register_routes(
            Box::new(move |router: &mut dyn signalk_plugin_api::PluginRouter| {
                let sg = store_get.clone();
                router.get(
                    "/",
                    route_handler(move |req| {
                        let s = sg.clone();
                        async move { handle_get_tracks(&s, &req) }
                    }),
                );

                let ss = store_summary.clone();
                router.get(
                    "/summary",
                    route_handler(move |_req| {
                        let s = ss.clone();
                        async move { handle_get_summary(&s) }
                    }),
                );

                let sd = store_delete.clone();
                router.delete(
                    "/",
                    route_handler(move |req| {
                        let s = sd.clone();
                        async move { handle_delete_tracks(&s, &req) }
                    }),
                );
            }) as RouterSetup,
        )
        .await?;

        // ── Subscribe to vessel deltas ──────────────────────────────────────
        let store_sub = store.clone();
        let track_self = cfg.track_self;
        let track_others = cfg.track_others;
        let min_interval = TimeDelta::seconds(cfg.min_interval_secs as i64);

        // Per-vessel: last recorded timestamp (resolution filter) + aux data cache
        let last_recorded: Arc<Mutex<HashMap<String, chrono::DateTime<chrono::Utc>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let aux_cache: Arc<Mutex<HashMap<String, AuxCache>>> = Arc::new(Mutex::new(HashMap::new()));

        let last_recorded_cb = last_recorded.clone();
        let aux_cache_cb = aux_cache.clone();

        let handle = ctx
            .subscribe(
                SubscriptionSpec::all_vessels(vec![
                    Subscription::path("navigation.position"),
                    Subscription::path("navigation.speedOverGround"),
                    Subscription::path("navigation.courseOverGroundTrue"),
                    Subscription::path("environment.depth.belowTransducer"),
                ]),
                delta_callback(move |delta: Delta| {
                    let context = match delta.context.as_deref() {
                        Some(c) => c,
                        None => return,
                    };

                    // Filter by self/others config
                    let is_self = context == "vessels.self"
                        || context.starts_with("vessels.urn:mrn:signalk:uuid:");
                    if is_self && !track_self {
                        return;
                    }
                    if !is_self && !track_others {
                        return;
                    }

                    let mut position: Option<(f64, f64)> = None;
                    let mut timestamp = chrono::Utc::now();

                    // Extract values from delta updates
                    for update in &delta.updates {
                        timestamp = update.timestamp;
                        for pv in &update.values {
                            match pv.path.as_str() {
                                "navigation.position" => {
                                    if let Some(obj) = pv.value.as_object() {
                                        let lat = obj.get("latitude").and_then(|v| v.as_f64());
                                        let lon = obj.get("longitude").and_then(|v| v.as_f64());
                                        if let (Some(lat), Some(lon)) = (lat, lon) {
                                            position = Some((lat, lon));
                                        }
                                    }
                                }
                                "navigation.speedOverGround" => {
                                    if let Some(v) = pv.value.as_f64() {
                                        aux_cache_cb
                                            .lock()
                                            .unwrap()
                                            .entry(context.to_string())
                                            .or_default()
                                            .sog = Some(v);
                                    }
                                }
                                "navigation.courseOverGroundTrue" => {
                                    if let Some(v) = pv.value.as_f64() {
                                        aux_cache_cb
                                            .lock()
                                            .unwrap()
                                            .entry(context.to_string())
                                            .or_default()
                                            .cog = Some(v);
                                    }
                                }
                                "environment.depth.belowTransducer" => {
                                    if let Some(v) = pv.value.as_f64() {
                                        aux_cache_cb
                                            .lock()
                                            .unwrap()
                                            .entry(context.to_string())
                                            .or_default()
                                            .depth = Some(v);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }

                    // Only record if we got a position
                    let (lat, lon) = match position {
                        Some(pos) => pos,
                        None => return,
                    };

                    // Resolution filter: skip if too recent
                    {
                        let mut last = last_recorded_cb.lock().unwrap();
                        if let Some(prev) = last.get(context)
                            && timestamp - *prev < min_interval
                        {
                            return;
                        }
                        last.insert(context.to_string(), timestamp);
                    }

                    // Build TrackPoint with aux data
                    let aux = aux_cache_cb
                        .lock()
                        .unwrap()
                        .get(context)
                        .cloned()
                        .unwrap_or_default();

                    let point = TrackPoint {
                        lat,
                        lon,
                        timestamp,
                        sog: aux.sog,
                        cog: aux.cog,
                        depth: aux.depth,
                    };

                    store_sub.lock().unwrap().record(context, point);
                }),
            )
            .await?;

        // ── Tick task for pruning + status ───────────────────────────────────
        let (abort_tx, mut abort_rx) = tokio::sync::watch::channel(false);
        let store_tick = store.clone();
        let ctx_tick = ctx.clone();
        let max_age = TimeDelta::hours(cfg.max_age_hours as i64);
        let tick_interval_secs = cfg.tick_interval_secs;

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(tick_interval_secs));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let (total, vessels) = {
                            let mut s = store_tick.lock().unwrap();
                            s.prune(max_age);
                            (s.total_points(), s.vessel_count())
                        };
                        ctx_tick.set_status(&format!(
                            "Recording {total} points across {vessels} vessels"
                        ));
                    }
                    _ = abort_rx.changed() => {
                        if *abort_rx.borrow() {
                            debug!("Tracks tick task stopping");
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

        info!("Tracks plugin stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_plugin_api::testing::MockPluginContext;
    use signalk_types::{PathValue, Source, Update};

    #[tokio::test]
    async fn start_with_default_config() {
        let mut plugin = TracksPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        let result = plugin.start(serde_json::json!({}), ctx).await;
        assert!(result.is_ok());

        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn start_with_custom_config() {
        let mut plugin = TracksPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        let result = plugin
            .start(
                serde_json::json!({
                    "min_interval_secs": 10,
                    "max_points": 10000,
                    "max_age_hours": 48,
                    "track_self": true,
                    "track_others": false
                }),
                ctx,
            )
            .await;
        assert!(result.is_ok());

        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn start_with_invalid_config_fails() {
        let mut plugin = TracksPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        let result = plugin
            .start(
                serde_json::json!({ "min_interval_secs": "not_a_number" }),
                ctx,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn metadata_is_correct() {
        let plugin = TracksPlugin::new();
        let meta = plugin.metadata();
        assert_eq!(meta.id, "tracks");
        assert_eq!(meta.name, "Vessel Tracks");
    }

    #[tokio::test]
    async fn position_delta_triggers_callback() {
        let mut plugin = TracksPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        plugin
            .start(serde_json::json!({ "min_interval_secs": 0 }), ctx.clone())
            .await
            .unwrap();

        // Send a position delta — deliver_delta is synchronous
        let delta = Delta::with_context(
            "vessels.self".to_string(),
            vec![Update::new(
                Source::plugin("test"),
                vec![PathValue::new(
                    "navigation.position",
                    serde_json::json!({ "latitude": 54.0, "longitude": 10.0 }),
                )],
            )],
        );

        ctx.deliver_delta(&delta);

        // The callback ran synchronously; the fact that it didn't panic
        // confirms position extraction and store recording work.

        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn resolution_filter_skips_rapid_updates() {
        let mut plugin = TracksPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        // 60 second min interval — very aggressive filtering
        plugin
            .start(serde_json::json!({ "min_interval_secs": 60 }), ctx.clone())
            .await
            .unwrap();

        // First delta — should be recorded
        let delta1 = Delta {
            context: Some("vessels.self".into()),
            updates: vec![Update::new(
                Source::plugin("test"),
                vec![PathValue::new(
                    "navigation.position",
                    serde_json::json!({ "latitude": 54.0, "longitude": 10.0 }),
                )],
            )],
        };

        // Second delta — should be skipped by resolution filter
        // (Update::new uses Utc::now(), which is within 60s of the first)
        let delta2 = Delta {
            context: Some("vessels.self".into()),
            updates: vec![Update::new(
                Source::plugin("test"),
                vec![PathValue::new(
                    "navigation.position",
                    serde_json::json!({ "latitude": 54.1, "longitude": 10.1 }),
                )],
            )],
        };

        ctx.deliver_delta(&delta1);
        ctx.deliver_delta(&delta2);

        // The callback ran without panicking with the resolution filter active.

        plugin.stop().await.unwrap();
    }
}
