/// Anchor alarm plugin for signalk-rs.
///
/// Subscribes to `navigation.position` and checks whether the vessel has
/// drifted beyond a configurable radius from the anchor drop point.
/// When the vessel exits the radius, a notification is emitted via
/// `notifications.navigation.anchor`.
///
/// Config:
/// ```json
/// {
///   "position": { "latitude": 49.2744, "longitude": -123.1888 },
///   "radius": 75.0
/// }
/// ```
///
/// - `position`: the anchor drop point (lat/lon in degrees)
/// - `radius`: maximum allowed drift in meters (default: 50.0)
use async_trait::async_trait;
use serde::Deserialize;
use signalk_plugin_api::{
    Plugin, PluginContext, PluginError, PluginMetadata, SubscriptionHandle, SubscriptionSpec,
    delta_callback,
};
use signalk_types::{Delta, Notification, NotificationMethod, NotificationState, Subscription};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

// ─── Config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct AnchorConfig {
    position: AnchorPosition,
    #[serde(default = "default_radius")]
    radius: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct AnchorPosition {
    latitude: f64,
    longitude: f64,
}

fn default_radius() -> f64 {
    50.0
}

// ─── Plugin ─────────────────────────────────────────────────────────────────

pub struct AnchorAlarmPlugin {
    subscription_handle: Option<SubscriptionHandle>,
    ctx: Option<Arc<dyn PluginContext>>,
}

impl AnchorAlarmPlugin {
    pub fn new() -> Self {
        AnchorAlarmPlugin {
            subscription_handle: None,
            ctx: None,
        }
    }
}

impl Default for AnchorAlarmPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for AnchorAlarmPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "anchor-alarm",
            "Anchor Alarm",
            "Alerts when vessel drifts beyond anchor radius",
            "0.1.0",
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "required": ["position", "radius"],
            "properties": {
                "position": {
                    "type": "object",
                    "required": ["latitude", "longitude"],
                    "properties": {
                        "latitude": { "type": "number", "description": "Anchor latitude (degrees)" },
                        "longitude": { "type": "number", "description": "Anchor longitude (degrees)" }
                    }
                },
                "radius": {
                    "type": "number",
                    "description": "Maximum drift radius in meters",
                    "default": 50.0
                }
            }
        }))
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let anchor_config: AnchorConfig = serde_json::from_value(config)
            .map_err(|e| PluginError::config(format!("invalid anchor config: {e}")))?;

        let radius = anchor_config.radius;
        let anchor_lat = anchor_config.position.latitude;
        let anchor_lon = anchor_config.position.longitude;

        info!(
            lat = anchor_lat,
            lon = anchor_lon,
            radius,
            "Anchor alarm set"
        );

        ctx.set_status(&format!(
            "Monitoring: radius {radius:.0}m at ({anchor_lat:.4}, {anchor_lon:.4})"
        ));

        let alarming = Arc::new(Mutex::new(false));
        let alarming_clone = alarming.clone();
        let ctx_clone = ctx.clone();

        let handle = ctx
            .subscribe(
                SubscriptionSpec::self_vessel(vec![Subscription::path("navigation.position")]),
                delta_callback(move |delta: Delta| {
                    for update in &delta.updates {
                        for pv in &update.values {
                            if pv.path != "navigation.position" {
                                continue;
                            }

                            let Some(lat) = pv.value.get("latitude").and_then(|v| v.as_f64())
                            else {
                                continue;
                            };
                            let Some(lon) = pv.value.get("longitude").and_then(|v| v.as_f64())
                            else {
                                continue;
                            };

                            let distance = haversine_meters(anchor_lat, anchor_lon, lat, lon);
                            debug!(distance, radius, "Anchor distance check");

                            let was_alarming = *alarming_clone.lock().unwrap();
                            let is_outside = distance > radius;

                            if is_outside && !was_alarming {
                                warn!(distance, radius, "Anchor alarm triggered!");
                                *alarming_clone.lock().unwrap() = true;
                                emit_notification(&ctx_clone, distance, radius, true);
                            } else if !is_outside && was_alarming {
                                info!(distance, radius, "Back inside anchor radius");
                                *alarming_clone.lock().unwrap() = false;
                                emit_notification(&ctx_clone, distance, radius, false);
                            }
                        }
                    }
                }),
            )
            .await?;

        self.subscription_handle = Some(handle);
        self.ctx = Some(ctx);

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        if let (Some(handle), Some(ctx)) = (self.subscription_handle.take(), self.ctx.take()) {
            ctx.unsubscribe(handle).await?;
        }
        Ok(())
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Haversine distance in meters between two lat/lon points (in degrees).
fn haversine_meters(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0; // Earth radius in meters
    let d_lat = (lat2 - lat1).to_radians();
    let d_lon = (lon2 - lon1).to_radians();
    let a = (d_lat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    R * c
}

/// Emit a SignalK notification via `raise_notification`.
///
/// Spawns a `tokio` task to call the async method from within
/// the synchronous subscription callback.
fn emit_notification(ctx: &Arc<dyn PluginContext>, distance: f64, radius: f64, alarming: bool) {
    let (state, method, message) = if alarming {
        (
            NotificationState::Alarm,
            vec![NotificationMethod::Visual, NotificationMethod::Sound],
            format!("Anchor alarm! Vessel is {distance:.0}m from anchor (radius: {radius:.0}m)"),
        )
    } else {
        (
            NotificationState::Normal,
            vec![],
            format!("Back within anchor radius ({distance:.0}m, radius: {radius:.0}m)"),
        )
    };

    let notification = Notification {
        state,
        method,
        message,
    };

    let ctx = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = ctx
            .raise_notification("navigation.anchor", notification, "anchor-alarm")
            .await
        {
            tracing::error!("Failed to emit anchor notification: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_types::{PathValue, Source, Update};

    #[test]
    fn haversine_zero_distance() {
        let d = haversine_meters(49.2744, -123.1888, 49.2744, -123.1888);
        assert!(d.abs() < 0.01);
    }

    #[test]
    fn haversine_known_distance() {
        // Vancouver (49.2827, -123.1207) to North Van (49.3200, -123.0724)
        // ~5.5 km
        let d = haversine_meters(49.2827, -123.1207, 49.3200, -123.0724);
        assert!(d > 4000.0 && d < 7000.0, "distance was {d}");
    }

    #[test]
    fn metadata_id() {
        let plugin = AnchorAlarmPlugin::new();
        assert_eq!(plugin.metadata().id, "anchor-alarm");
    }

    #[tokio::test]
    async fn start_with_valid_config() {
        use signalk_plugin_api::testing::MockPluginContext;

        let mut plugin = AnchorAlarmPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        let result = plugin
            .start(
                serde_json::json!({
                    "position": { "latitude": 49.2744, "longitude": -123.1888 },
                    "radius": 75.0
                }),
                ctx,
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn start_rejects_missing_position() {
        use signalk_plugin_api::testing::MockPluginContext;

        let mut plugin = AnchorAlarmPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        let result = plugin.start(serde_json::json!({}), ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn alarm_triggers_on_drift() {
        use signalk_plugin_api::testing::MockPluginContext;

        let mut plugin = AnchorAlarmPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        plugin
            .start(
                serde_json::json!({
                    "position": { "latitude": 49.2744, "longitude": -123.1888 },
                    "radius": 50.0
                }),
                ctx.clone(),
            )
            .await
            .unwrap();

        // Simulate a position update that's far from anchor (1 degree away ≈ 111km)
        let position_delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![PathValue::new(
                "navigation.position",
                serde_json::json!({ "latitude": 50.2744, "longitude": -123.1888 }),
            )],
        )]);

        ctx.deliver_delta(&position_delta);

        // Give the subscription callback time to fire
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // The notification should have been emitted via handle_message
        let deltas = ctx.emitted_deltas.lock().unwrap();
        assert!(
            deltas.iter().any(|d| {
                d.updates.iter().any(|u| {
                    u.values
                        .iter()
                        .any(|pv| pv.path == "notifications.navigation.anchor")
                })
            }),
            "Expected anchor alarm notification, got: {:?}",
            *deltas
        );
    }
}
