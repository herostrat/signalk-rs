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
use signalk_plugin_api::{Plugin, PluginContext, PluginError, PluginMetadata};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

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
        Some(serde_json::json!({
            "type": "object",
            "required": ["interface"],
            "properties": {
                "interface": {
                    "type": "string",
                    "description": "Interface: CAN interface (can0) for socketcan, serial port (/dev/ttyUSB0) for slcan/actisense",
                    "default": "can0"
                },
                "transport": {
                    "type": "string",
                    "description": "Transport type: socketcan, slcan, or actisense",
                    "default": "socketcan",
                    "enum": ["socketcan", "slcan", "actisense"]
                },
                "source_label": {
                    "type": "string",
                    "description": "Source label for SignalK deltas",
                    "default": "nmea2000"
                }
            }
        }))
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let interface = config["interface"]
            .as_str()
            .ok_or_else(|| PluginError::config("missing 'interface'"))?
            .to_string();

        let transport = config["transport"]
            .as_str()
            .unwrap_or("socketcan")
            .to_string();

        let source_label = config["source_label"]
            .as_str()
            .unwrap_or("nmea2000")
            .to_string();

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
                        debug!("N2k read error: {e:?}");
                        break;
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
    async fn plugin_rejects_missing_interface() {
        use signalk_plugin_api::testing::MockPluginContext;

        let mut plugin = Nmea2000Plugin::new();
        let ctx = Arc::new(MockPluginContext::new());
        let result = plugin.start(serde_json::json!({}), ctx).await;
        assert!(result.is_err());
    }

    #[test]
    fn default_transport_is_socketcan() {
        let config = serde_json::json!({"interface": "can0"});
        let transport = config["transport"].as_str().unwrap_or("socketcan");
        assert_eq!(transport, "socketcan");
    }
}
