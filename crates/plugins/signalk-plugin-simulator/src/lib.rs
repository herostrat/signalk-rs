/// Simulator plugin for signalk-rs.
///
/// Generates realistic SignalK sensor data for development and testing.
/// Only produces **direct sensor measurements** — derived values (true wind,
/// true heading, STW, etc.) are left to `signalk-derived-data`.
///
/// Guarded by the `simulator` feature flag in signalk-server so it never
/// ships in release builds.
///
/// Config:
/// ```json
/// {
///   "update_interval_ms": 1000,
///   "position": { "latitude": 54.5, "longitude": 10.0 },
///   "orbit_radius_m": 200,
///   "orbit_period_s": 300,
///   "magnetic_variation_deg": 2.5,
///   "enable_propulsion": true,
///   "enable_environment": true
/// }
/// ```
use async_trait::async_trait;
use serde::Deserialize;
use signalk_plugin_api::{Plugin, PluginContext, PluginError, PluginMetadata};
use signalk_types::{Delta, PathValue, Source, Update};
use std::sync::Arc;
use tokio::task::AbortHandle;
use tracing::info;

mod generators;
use generators::SimulatorState;

// ─── Config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct SimulatorConfig {
    #[serde(default = "default_interval")]
    update_interval_ms: u64,

    #[serde(default = "default_position")]
    position: PositionConfig,

    #[serde(default = "default_orbit_radius")]
    orbit_radius_m: f64,

    #[serde(default = "default_orbit_period")]
    orbit_period_s: f64,

    #[serde(default = "default_variation")]
    magnetic_variation_deg: f64,

    #[serde(default = "default_true")]
    enable_propulsion: bool,

    #[serde(default = "default_true")]
    enable_environment: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct PositionConfig {
    #[serde(default = "default_lat")]
    latitude: f64,
    #[serde(default = "default_lon")]
    longitude: f64,
}

fn default_interval() -> u64 {
    1000
}
fn default_position() -> PositionConfig {
    PositionConfig {
        latitude: default_lat(),
        longitude: default_lon(),
    }
}
fn default_lat() -> f64 {
    54.5
}
fn default_lon() -> f64 {
    10.0
}
fn default_orbit_radius() -> f64 {
    200.0
}
fn default_orbit_period() -> f64 {
    300.0
}
fn default_variation() -> f64 {
    2.5
}
fn default_true() -> bool {
    true
}

impl Default for SimulatorConfig {
    fn default() -> Self {
        SimulatorConfig {
            update_interval_ms: default_interval(),
            position: default_position(),
            orbit_radius_m: default_orbit_radius(),
            orbit_period_s: default_orbit_period(),
            magnetic_variation_deg: default_variation(),
            enable_propulsion: true,
            enable_environment: true,
        }
    }
}

// ─── Plugin ─────────────────────────────────────────────────────────────────

pub struct SimulatorPlugin {
    abort_handle: Option<AbortHandle>,
}

impl SimulatorPlugin {
    pub fn new() -> Self {
        SimulatorPlugin { abort_handle: None }
    }
}

impl Default for SimulatorPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for SimulatorPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "simulator",
            "Simulator",
            "Generates realistic SignalK sensor data for development",
            "0.1.0",
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "update_interval_ms": {
                    "type": "integer",
                    "description": "Delta generation interval in milliseconds",
                    "default": 1000,
                    "minimum": 100
                },
                "position": {
                    "type": "object",
                    "properties": {
                        "latitude": { "type": "number", "description": "Center latitude (degrees)", "default": 54.5 },
                        "longitude": { "type": "number", "description": "Center longitude (degrees)", "default": 10.0 }
                    }
                },
                "orbit_radius_m": {
                    "type": "number",
                    "description": "Circular orbit radius in meters",
                    "default": 200
                },
                "orbit_period_s": {
                    "type": "number",
                    "description": "Time for one full circle in seconds",
                    "default": 300
                },
                "magnetic_variation_deg": {
                    "type": "number",
                    "description": "Local magnetic variation in degrees",
                    "default": 2.5
                },
                "enable_propulsion": {
                    "type": "boolean",
                    "description": "Simulate engine data (RPM, temperatures)",
                    "default": true
                },
                "enable_environment": {
                    "type": "boolean",
                    "description": "Simulate environmental data (wind, depth, temperature, pressure)",
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
        let sim_config: SimulatorConfig = serde_json::from_value(config)
            .map_err(|e| PluginError::config(format!("invalid simulator config: {e}")))?;

        info!(
            interval_ms = sim_config.update_interval_ms,
            lat = sim_config.position.latitude,
            lon = sim_config.position.longitude,
            radius_m = sim_config.orbit_radius_m,
            period_s = sim_config.orbit_period_s,
            propulsion = sim_config.enable_propulsion,
            environment = sim_config.enable_environment,
            "Simulator starting"
        );

        let state = SimulatorState::new(
            sim_config.position.latitude,
            sim_config.position.longitude,
            sim_config.orbit_radius_m,
            sim_config.orbit_period_s,
            sim_config.magnetic_variation_deg,
            sim_config.enable_propulsion,
        );

        let interval = tokio::time::Duration::from_millis(sim_config.update_interval_ms);
        let enable_env = sim_config.enable_environment;

        let ctx_for_spawn = ctx.clone();
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let values = state.tick();
                let delta = build_delta(&values, enable_env);

                if let Err(e) = ctx_for_spawn.handle_message(delta).await {
                    tracing::warn!(error = %e, "Simulator: failed to emit delta");
                }
            }
        });

        self.abort_handle = Some(handle.abort_handle());
        ctx.set_status("Generating data");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(handle) = self.abort_handle.take() {
            handle.abort();
        }
        Ok(())
    }
}

// ─── Delta builder ──────────────────────────────────────────────────────────

fn build_delta(values: &generators::SimulatedValues, enable_environment: bool) -> Delta {
    let source = Source::plugin("simulator");
    let mut path_values = Vec::with_capacity(30);

    // Navigation (always included)
    path_values.push(PathValue::new(
        "navigation.position",
        serde_json::json!({
            "latitude": values.latitude,
            "longitude": values.longitude
        }),
    ));
    path_values.push(PathValue::new(
        "navigation.speedOverGround",
        serde_json::json!(values.sog_mps),
    ));
    path_values.push(PathValue::new(
        "navigation.courseOverGroundTrue",
        serde_json::json!(values.cog_rad),
    ));
    path_values.push(PathValue::new(
        "navigation.headingMagnetic",
        serde_json::json!(values.heading_magnetic_rad),
    ));
    path_values.push(PathValue::new(
        "navigation.magneticVariation",
        serde_json::json!(values.magnetic_variation_rad),
    ));
    path_values.push(PathValue::new(
        "navigation.speedThroughWater",
        serde_json::json!(values.stw_mps),
    ));
    path_values.push(PathValue::new(
        "navigation.attitude",
        serde_json::json!({
            "roll": values.roll_rad,
            "pitch": values.pitch_rad,
            "yaw": 0.0
        }),
    ));
    path_values.push(PathValue::new(
        "navigation.datetime",
        serde_json::json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
    ));

    // Environment (optional)
    if enable_environment {
        path_values.push(PathValue::new(
            "environment.wind.angleApparent",
            serde_json::json!(values.wind_angle_apparent_rad),
        ));
        path_values.push(PathValue::new(
            "environment.wind.speedApparent",
            serde_json::json!(values.wind_speed_apparent_mps),
        ));
        path_values.push(PathValue::new(
            "environment.depth.belowTransducer",
            serde_json::json!(values.depth_below_transducer_m),
        ));
        path_values.push(PathValue::new(
            "environment.depth.surfaceToTransducer",
            serde_json::json!(values.surface_to_transducer_m),
        ));
        path_values.push(PathValue::new(
            "environment.water.temperature",
            serde_json::json!(values.water_temperature_k),
        ));
        path_values.push(PathValue::new(
            "environment.outside.temperature",
            serde_json::json!(values.air_temperature_k),
        ));
        path_values.push(PathValue::new(
            "environment.outside.pressure",
            serde_json::json!(values.pressure_pa),
        ));
        path_values.push(PathValue::new(
            "environment.outside.humidity",
            serde_json::json!(values.humidity_ratio),
        ));
    }

    // Propulsion + electrical + fuel (optional)
    if let Some(ref prop) = values.propulsion {
        path_values.push(PathValue::new(
            "propulsion.main.revolutions",
            serde_json::json!(prop.revolutions_hz),
        ));
        path_values.push(PathValue::new(
            "propulsion.main.oilTemperature",
            serde_json::json!(prop.oil_temperature_k),
        ));
        path_values.push(PathValue::new(
            "propulsion.main.coolantTemperature",
            serde_json::json!(prop.coolant_temperature_k),
        ));
        path_values.push(PathValue::new(
            "propulsion.main.fuel.rate",
            serde_json::json!(prop.fuel_rate_m3s),
        ));
        path_values.push(PathValue::new(
            "electrical.batteries.0.voltage",
            serde_json::json!(prop.battery_voltage),
        ));
        path_values.push(PathValue::new(
            "electrical.batteries.0.current",
            serde_json::json!(prop.battery_current),
        ));
    }

    // Tanks (always included)
    path_values.push(PathValue::new(
        "tanks.fuel.0.currentLevel",
        serde_json::json!(values.fuel_tank_level),
    ));
    path_values.push(PathValue::new(
        "tanks.fuel.0.capacity",
        serde_json::json!(values.fuel_tank_capacity_m3),
    ));
    path_values.push(PathValue::new(
        "tanks.freshWater.0.currentLevel",
        serde_json::json!(values.fresh_water_level),
    ));
    path_values.push(PathValue::new(
        "tanks.freshWater.0.capacity",
        serde_json::json!(values.fresh_water_capacity_m3),
    ));

    Delta::self_vessel(vec![Update::new(source, path_values)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_plugin_api::testing::MockPluginContext;

    #[test]
    fn metadata_id() {
        let plugin = SimulatorPlugin::new();
        assert_eq!(plugin.metadata().id, "simulator");
    }

    #[test]
    fn default_config_deserializes() {
        let config: SimulatorConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(config.update_interval_ms, 1000);
        assert_eq!(config.position.latitude, 54.5);
        assert!(config.enable_propulsion);
        assert!(config.enable_environment);
    }

    #[test]
    fn custom_config_deserializes() {
        let config: SimulatorConfig = serde_json::from_value(serde_json::json!({
            "update_interval_ms": 500,
            "position": { "latitude": 48.0, "longitude": -3.0 },
            "orbit_radius_m": 500,
            "enable_propulsion": false
        }))
        .unwrap();
        assert_eq!(config.update_interval_ms, 500);
        assert_eq!(config.position.latitude, 48.0);
        assert_eq!(config.orbit_radius_m, 500.0);
        assert!(!config.enable_propulsion);
        assert!(config.enable_environment); // still default
    }

    #[test]
    fn build_delta_contains_navigation() {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, false);
        let values = state.tick();
        let delta = build_delta(&values, false);

        let paths: Vec<&str> = delta.updates[0]
            .values
            .iter()
            .map(|pv| pv.path.as_str())
            .collect();

        assert!(paths.contains(&"navigation.position"));
        assert!(paths.contains(&"navigation.speedOverGround"));
        assert!(paths.contains(&"navigation.speedThroughWater"));
        assert!(paths.contains(&"navigation.courseOverGroundTrue"));
        assert!(paths.contains(&"navigation.headingMagnetic"));
        assert!(paths.contains(&"navigation.magneticVariation"));
        assert!(paths.contains(&"navigation.attitude"));
        assert!(paths.contains(&"navigation.datetime"));
        // No environment or propulsion (but tanks are always present)
        assert!(!paths.iter().any(|p| p.starts_with("environment.")));
        assert!(!paths.iter().any(|p| p.starts_with("propulsion.")));
        assert!(paths.contains(&"tanks.fuel.0.currentLevel"));
    }

    #[test]
    fn build_delta_with_environment() {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, false);
        let values = state.tick();
        let delta = build_delta(&values, true);

        let paths: Vec<&str> = delta.updates[0]
            .values
            .iter()
            .map(|pv| pv.path.as_str())
            .collect();

        assert!(paths.contains(&"environment.wind.angleApparent"));
        assert!(paths.contains(&"environment.wind.speedApparent"));
        assert!(paths.contains(&"environment.depth.belowTransducer"));
        assert!(paths.contains(&"environment.depth.surfaceToTransducer"));
        assert!(paths.contains(&"environment.water.temperature"));
        assert!(paths.contains(&"environment.outside.temperature"));
        assert!(paths.contains(&"environment.outside.pressure"));
        assert!(paths.contains(&"environment.outside.humidity"));
    }

    #[test]
    fn build_delta_with_propulsion() {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, true);
        let values = state.tick();
        let delta = build_delta(&values, false);

        let paths: Vec<&str> = delta.updates[0]
            .values
            .iter()
            .map(|pv| pv.path.as_str())
            .collect();

        assert!(paths.contains(&"propulsion.main.revolutions"));
        assert!(paths.contains(&"propulsion.main.oilTemperature"));
        assert!(paths.contains(&"propulsion.main.coolantTemperature"));
        assert!(paths.contains(&"propulsion.main.fuel.rate"));
        assert!(paths.contains(&"electrical.batteries.0.voltage"));
        assert!(paths.contains(&"electrical.batteries.0.current"));
    }

    #[test]
    fn build_delta_position_has_lat_lon() {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, false);
        let values = state.tick();
        let delta = build_delta(&values, false);

        let pos_pv = delta.updates[0]
            .values
            .iter()
            .find(|pv| pv.path == "navigation.position")
            .unwrap();

        assert!(pos_pv.value.get("latitude").is_some());
        assert!(pos_pv.value.get("longitude").is_some());
    }

    #[tokio::test]
    async fn start_with_default_config() {
        let mut plugin = SimulatorPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        let result = plugin.start(serde_json::json!({}), ctx.clone()).await;
        assert!(result.is_ok());

        // Let it run for a couple ticks
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // Should have emitted at least one delta
        // (interval is 1000ms by default, but the first tick is immediate)
        {
            let deltas = ctx.emitted_deltas.lock().unwrap();
            assert!(!deltas.is_empty(), "expected at least one delta emitted");
        }

        // Stop
        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn start_with_fast_interval() {
        let mut plugin = SimulatorPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        plugin
            .start(
                serde_json::json!({ "update_interval_ms": 100 }),
                ctx.clone(),
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;

        let count = ctx.emitted_deltas.lock().unwrap().len();
        assert!(count >= 3, "expected >= 3 deltas, got {count}");

        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn stop_halts_generation() {
        let mut plugin = SimulatorPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        plugin
            .start(
                serde_json::json!({ "update_interval_ms": 100 }),
                ctx.clone(),
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        plugin.stop().await.unwrap();

        let count_after_stop = ctx.emitted_deltas.lock().unwrap().len();
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        let count_later = ctx.emitted_deltas.lock().unwrap().len();

        assert_eq!(
            count_after_stop, count_later,
            "deltas should stop after stop()"
        );
    }
}
