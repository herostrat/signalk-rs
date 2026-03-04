/// NMEA 2000 input plugin for signalk-rs.
///
/// Reads from SocketCAN, SLCAN, or Actisense and converts PGNs to SignalK deltas.
/// Navigation PGNs (heading, position, COG/SOG, depth, wind) produce
/// self-vessel deltas. AIS PGNs produce vessel-context deltas via
/// `AisContact::to_delta()`.
///
/// Config example (signalk-rs.toml):
/// ```toml
/// [[plugins]]
/// id = "nmea2000"
/// enabled = true
/// config = { interface = "can0", transport = "socketcan" }
/// ```
pub mod pgn;

use async_trait::async_trait;
use nmea2000::N2kTransport;
use serde::Deserialize;
use signalk_plugin_api::{Plugin, PluginContext, PluginError, PluginMetadata};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

// ─── Config struct ──────────────────────────────────────────────────────────

/// Configuration for the NMEA 2000 input plugin.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[schemars(default)]
pub struct N2kConfig {
    /// Interface: CAN interface (can0) for socketcan, serial port (/dev/ttyUSB0) for slcan/actisense.
    #[serde(default = "default_interface")]
    pub interface: String,
    /// Transport type: socketcan, slcan, or actisense.
    #[serde(default = "default_transport")]
    pub transport: String,
    /// Source label for SignalK deltas.
    #[serde(default = "default_n2k_source_label")]
    pub source_label: String,
}

impl Default for N2kConfig {
    fn default() -> Self {
        N2kConfig {
            interface: default_interface(),
            transport: default_transport(),
            source_label: default_n2k_source_label(),
        }
    }
}

fn default_interface() -> String {
    "can0".to_string()
}

fn default_transport() -> String {
    "socketcan".to_string()
}

fn default_n2k_source_label() -> String {
    "nmea2000".to_string()
}

pub struct Nmea2000Plugin {
    abort_handle: Option<tokio::task::AbortHandle>,
}

impl Nmea2000Plugin {
    pub fn new() -> Self {
        Nmea2000Plugin { abort_handle: None }
    }
}

impl Default for Nmea2000Plugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for Nmea2000Plugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "nmea2000",
            "NMEA 2000",
            "SocketCAN/SLCAN/Actisense input for NMEA 2000 PGNs",
            env!("CARGO_PKG_VERSION"),
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::to_value(schemars::schema_for!(N2kConfig)).unwrap())
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let cfg: N2kConfig =
            serde_json::from_value(config).map_err(|e| PluginError::config(format!("{e}")))?;

        let interface = cfg.interface;
        let transport = cfg.transport;
        let source_label = cfg.source_label;

        info!(interface = %interface, transport = %transport, "NMEA 2000 plugin starting");

        let handle = tokio::spawn(async move {
            run_n2k_reader(&interface, &transport, &source_label, ctx).await;
        })
        .abort_handle();

        self.abort_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(h) = self.abort_handle.take() {
            h.abort();
        }
        info!("NMEA 2000 plugin stopped");
        Ok(())
    }
}

/// Open the configured transport, returning a boxed `N2kTransport`.
fn open_transport(
    interface: &str,
    transport: &str,
) -> Result<Box<dyn N2kTransport + Send>, String> {
    match transport {
        "slcan" => {
            let bus = nmea2000::N2kBus::open_slcan(interface)
                .map_err(|e| format!("SLCAN open {interface}: {e}"))?;
            Ok(Box::new(bus))
        }
        "actisense" => {
            let t = nmea2000::ActisenseTransport::open(interface)
                .map_err(|e| format!("Actisense open {interface}: {e}"))?;
            Ok(Box::new(t))
        }
        _ => {
            let bus = nmea2000::N2kBus::open(interface)
                .map_err(|e| format!("SocketCAN open {interface}: {e}"))?;
            Ok(Box::new(bus))
        }
    }
}

/// Main reader loop: opens transport, reads messages, converts to deltas.
///
/// Uses `spawn_blocking` because all transports read synchronously.
/// Messages are passed from the blocking reader to the async handler via
/// an mpsc channel.
async fn run_n2k_reader(
    interface: &str,
    transport_type: &str,
    label: &str,
    ctx: Arc<dyn PluginContext>,
) {
    let label = label.to_string();

    loop {
        ctx.set_status(&format!("Opening {interface}..."));

        let iface = interface.to_string();
        let ttype = transport_type.to_string();
        let transport_result =
            tokio::task::spawn_blocking(move || open_transport(&iface, &ttype)).await;

        let mut transport = match transport_result {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                ctx.set_error(&e);
                error!("{e}");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
            Err(e) => {
                error!("spawn_blocking panicked: {e}");
                return;
            }
        };

        ctx.set_status(&format!("Connected to {interface}"));
        info!(interface = %interface, transport = %transport_type, "Transport opened");

        // Channel: blocking reader → async handler
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);

        let reader_handle = tokio::task::spawn_blocking(move || {
            loop {
                match transport.receive_message() {
                    Ok(raw) => {
                        if tx.blocking_send(raw).is_err() {
                            break; // receiver dropped
                        }
                    }
                    Err(e) => {
                        warn!("N2k read error: {e:?}");
                        // Transient errors (e.g. error frames) — continue reading.
                        // Fatal errors (interface gone) will fail again immediately.
                        if e.kind() == std::io::ErrorKind::NotFound
                            || e.kind() == std::io::ErrorKind::PermissionDenied
                            || e.raw_os_error() == Some(19) // ENODEV
                            || e.raw_os_error() == Some(6)
                        // ENXIO
                        {
                            break;
                        }
                    }
                }
            }
        });

        // Process messages from channel
        while let Some(raw) = rx.recv().await {
            match nmea2000::DecodedMessage::decode(raw.pgn.as_u32(), &raw.data) {
                Ok(decoded) => {
                    let source = pgn::N2kSource {
                        label: &label,
                        src: raw.source,
                        pgn: raw.pgn.as_u32(),
                    };
                    if let Some(delta) = pgn::decoded_to_delta(&decoded, &source) {
                        ctx.handle_message(delta).await.ok();
                    }
                }
                Err(e) => {
                    debug!(pgn = raw.pgn.as_u32(), "PGN decode error: {e:?}");
                }
            }
        }

        // Reader task finished — reconnect
        let _ = reader_handle.await;
        warn!(interface = %interface, "Connection lost — reconnecting in 5s");
        ctx.set_error(&format!("Connection lost to {interface}"));
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_metadata() {
        let plugin = Nmea2000Plugin::new();
        let meta = plugin.metadata();
        assert_eq!(meta.id, "nmea2000");
    }

    #[tokio::test]
    async fn plugin_rejects_invalid_config() {
        use signalk_plugin_api::testing::MockPluginContext;

        let mut plugin = Nmea2000Plugin::new();
        let ctx = Arc::new(MockPluginContext::new());
        // Wrong type for interface should fail deserialization
        let result = plugin
            .start(serde_json::json!({"interface": 42}), ctx)
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn default_transport_is_socketcan() {
        let cfg = N2kConfig::default();
        assert_eq!(cfg.transport, "socketcan");
        assert_eq!(cfg.interface, "can0");
        assert_eq!(cfg.source_label, "nmea2000");
    }
}
