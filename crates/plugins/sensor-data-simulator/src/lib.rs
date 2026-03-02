/// Simulator plugin for signalk-rs.
///
/// Generates realistic SignalK sensor data for development and testing.
/// Only produces **direct sensor measurements** — derived values (true wind,
/// true heading, STW, etc.) are left to `derived-data`.
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
use std::sync::Arc;
use tokio::task::AbortHandle;
use tracing::info;

mod generators;
mod output_direct;
mod output_nmea0183;
mod output_nmea2000;

use generators::SimulatorState;

// ─── Config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[schemars(default)]
struct SimulatorConfig {
    /// Delta generation interval in milliseconds.
    #[serde(default = "default_interval")]
    update_interval_ms: u64,

    /// Center position for the simulated orbit.
    #[serde(default = "default_position")]
    position: PositionConfig,

    /// Circular orbit radius in meters.
    #[serde(default = "default_orbit_radius")]
    orbit_radius_m: f64,

    /// Time for one full circle in seconds.
    #[serde(default = "default_orbit_period")]
    orbit_period_s: f64,

    /// Local magnetic variation in degrees.
    #[serde(default = "default_variation")]
    magnetic_variation_deg: f64,

    /// Simulate engine data (RPM, temperatures).
    #[serde(default = "default_true")]
    enable_propulsion: bool,

    /// Simulate environmental data (wind, depth, temperature, pressure).
    #[serde(default = "default_true")]
    enable_environment: bool,

    /// Output configuration.
    #[serde(default)]
    output: OutputConfig,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[schemars(default)]
struct OutputConfig {
    /// Emit SignalK deltas directly into the store.
    #[serde(default = "default_true")]
    direct: bool,

    /// NMEA 0183 TCP output configuration.
    #[serde(default)]
    nmea0183: Nmea0183Config,

    /// NMEA 2000 SocketCAN/vcan output configuration.
    #[serde(default)]
    nmea2000: Nmea2000Config,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[schemars(default)]
struct Nmea0183Config {
    /// Enable NMEA 0183 TCP output.
    #[serde(default)]
    enabled: bool,

    /// Host of nmea0183-receive TCP listener.
    #[serde(default = "default_nmea0183_host")]
    host: String,

    /// Port of nmea0183-receive TCP listener.
    #[serde(default = "default_nmea0183_port")]
    port: u16,

    /// NMEA talker ID (GP, GN, II, etc.).
    #[serde(default = "default_talker_id")]
    talker_id: String,
}

impl Default for Nmea0183Config {
    fn default() -> Self {
        Nmea0183Config {
            enabled: false,
            host: default_nmea0183_host(),
            port: default_nmea0183_port(),
            talker_id: default_talker_id(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[schemars(default)]
struct Nmea2000Config {
    /// Enable NMEA 2000 SocketCAN/vcan output.
    #[serde(default)]
    enabled: bool,

    /// CAN interface (vcan0 for testing).
    #[serde(default = "default_nmea2000_interface")]
    interface: String,

    /// Fixed source address (0-252).
    #[serde(default = "default_nmea2000_source")]
    source_address: u8,
}

impl Default for Nmea2000Config {
    fn default() -> Self {
        Nmea2000Config {
            enabled: false,
            interface: default_nmea2000_interface(),
            source_address: default_nmea2000_source(),
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        OutputConfig {
            direct: true,
            nmea0183: Nmea0183Config::default(),
            nmea2000: Nmea2000Config::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct PositionConfig {
    /// Center latitude (degrees).
    #[serde(default = "default_lat")]
    latitude: f64,
    /// Center longitude (degrees).
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
fn default_nmea0183_host() -> String {
    "127.0.0.1".to_string()
}
fn default_nmea0183_port() -> u16 {
    10110
}
fn default_talker_id() -> String {
    "GP".to_string()
}
fn default_nmea2000_interface() -> String {
    "vcan0".to_string()
}
fn default_nmea2000_source() -> u8 {
    42
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
            output: OutputConfig::default(),
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
            "sensor-data-simulator",
            "Sensor Data Simulator",
            "Generates realistic SignalK sensor data for development",
            "0.1.0",
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::to_value(schemars::schema_for!(SimulatorConfig)).unwrap())
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
        let output_direct_enabled = sim_config.output.direct;

        let nmea0183_output = if sim_config.output.nmea0183.enabled {
            let cfg = &sim_config.output.nmea0183;
            info!(host = %cfg.host, port = cfg.port, talker = %cfg.talker_id, "NMEA 0183 output enabled");
            Some(output_nmea0183::Nmea0183Output::new(
                cfg.host.clone(),
                cfg.port,
                cfg.talker_id.clone(),
                enable_env,
            ))
        } else {
            None
        };

        // NMEA 2000 output: blocking thread + mpsc channel
        let nmea2000_tx = if sim_config.output.nmea2000.enabled {
            let cfg = &sim_config.output.nmea2000;
            info!(interface = %cfg.interface, source = cfg.source_address, "NMEA 2000 output enabled");
            let (tx, rx) = std::sync::mpsc::channel::<Vec<output_nmea2000::EncodedPgn>>();
            let iface = cfg.interface.clone();
            let source = cfg.source_address;
            tokio::task::spawn_blocking(move || {
                output_nmea2000::run_bus_writer(&iface, source, rx);
            });
            Some(tx)
        } else {
            None
        };

        let ctx_for_spawn = ctx.clone();
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            let mut nmea0183 = nmea0183_output;
            let mut n2k_sid: u8 = 0;

            loop {
                ticker.tick().await;
                let values = state.tick();

                if output_direct_enabled {
                    let delta = output_direct::build_delta(&values, enable_env);
                    if let Err(e) = ctx_for_spawn.handle_message(delta).await {
                        tracing::warn!(error = %e, "Simulator: failed to emit delta");
                    }
                }

                if let Some(ref mut out) = nmea0183 {
                    out.send(&values).await;
                }

                if let Some(ref tx) = nmea2000_tx {
                    let pgns = output_nmea2000::encode(&values, &mut n2k_sid, enable_env);
                    let _ = tx.send(pgns);
                }
            }
        });

        self.abort_handle = Some(handle.abort_handle());

        let mut status_parts = Vec::new();
        if sim_config.output.direct {
            status_parts.push("direct");
        }
        if sim_config.output.nmea0183.enabled {
            status_parts.push("nmea0183");
        }
        if sim_config.output.nmea2000.enabled {
            status_parts.push("nmea2000");
        }
        ctx.set_status(&format!("Generating data ({})", status_parts.join(", ")));
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(handle) = self.abort_handle.take() {
            handle.abort();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_plugin_api::testing::MockPluginContext;

    #[test]
    fn metadata_id() {
        let plugin = SimulatorPlugin::new();
        assert_eq!(plugin.metadata().id, "sensor-data-simulator");
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
    fn output_config_defaults() {
        let config: SimulatorConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(config.output.direct);
        assert!(!config.output.nmea0183.enabled);
        assert_eq!(config.output.nmea0183.host, "127.0.0.1");
        assert_eq!(config.output.nmea0183.port, 10110);
        assert_eq!(config.output.nmea0183.talker_id, "GP");
        assert!(!config.output.nmea2000.enabled);
        assert_eq!(config.output.nmea2000.interface, "vcan0");
        assert_eq!(config.output.nmea2000.source_address, 42);
    }

    #[test]
    fn output_config_with_nmea2000() {
        let config: SimulatorConfig = serde_json::from_value(serde_json::json!({
            "output": {
                "nmea2000": {
                    "enabled": true,
                    "interface": "can0",
                    "source_address": 100
                }
            }
        }))
        .unwrap();
        assert!(config.output.nmea2000.enabled);
        assert_eq!(config.output.nmea2000.interface, "can0");
        assert_eq!(config.output.nmea2000.source_address, 100);
    }

    #[test]
    fn output_config_with_nmea0183() {
        let config: SimulatorConfig = serde_json::from_value(serde_json::json!({
            "output": {
                "direct": false,
                "nmea0183": {
                    "enabled": true,
                    "host": "192.168.1.10",
                    "port": 10111,
                    "talker_id": "II"
                }
            }
        }))
        .unwrap();
        assert!(!config.output.direct);
        assert!(config.output.nmea0183.enabled);
        assert_eq!(config.output.nmea0183.host, "192.168.1.10");
        assert_eq!(config.output.nmea0183.port, 10111);
        assert_eq!(config.output.nmea0183.talker_id, "II");
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
