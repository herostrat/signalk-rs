/// NMEA 2000 input plugin for signalk-rs.
///
/// Reads from a SocketCAN interface and converts PGNs to SignalK deltas.
/// Navigation PGNs (heading, position, COG/SOG, depth, wind) produce
/// self-vessel deltas. AIS PGNs produce vessel-context deltas via
/// `AisContact::to_delta()`.
///
/// Config example (signalk-rs.toml):
/// ```toml
/// [[plugins]]
/// id = "nmea2000"
/// enabled = true
/// config = { interface = "can0", source_label = "nmea2000" }
/// ```
pub mod pgn_convert;

use async_trait::async_trait;
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
            "SocketCAN input for NMEA 2000 PGNs",
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
                    "description": "SocketCAN interface (e.g. can0, vcan0)",
                    "default": "can0"
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

        let source_label = config["source_label"]
            .as_str()
            .unwrap_or("nmea2000")
            .to_string();

        info!(interface = %interface, "NMEA 2000 plugin starting");

        let handle = tokio::spawn(async move {
            run_n2k_reader(&interface, &source_label, ctx).await;
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

/// Main reader loop: opens SocketCAN, reads messages, converts to deltas.
///
/// Uses `spawn_blocking` because `N2kBus::read_message()` is synchronous.
/// Messages are passed from the blocking reader to the async handler via
/// an mpsc channel.
async fn run_n2k_reader(interface: &str, label: &str, ctx: Arc<dyn PluginContext>) {
    let label = label.to_string();

    loop {
        ctx.set_status(&format!("Opening {interface}..."));

        let iface = interface.to_string();
        let bus_result =
            tokio::task::spawn_blocking(move || nmea2000::N2kBus::open(&iface)).await;

        let mut bus = match bus_result {
            Ok(Ok(bus)) => bus,
            Ok(Err(e)) => {
                ctx.set_error(&format!("Failed to open {interface}: {e}"));
                error!(interface = %interface, "SocketCAN open failed: {e}");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
            Err(e) => {
                error!("spawn_blocking panicked: {e}");
                return;
            }
        };

        ctx.set_status(&format!("Connected to {interface}"));
        info!(interface = %interface, "SocketCAN bus opened");

        // Channel: blocking reader → async handler
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);

        let reader_handle = tokio::task::spawn_blocking(move || {
            loop {
                match bus.read_message() {
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
                    let source = pgn_convert::N2kSource {
                        label: &label,
                        src: raw.source,
                        pgn: raw.pgn.as_u32(),
                    };
                    if let Some(delta) = pgn_convert::decoded_to_delta(&decoded, &source) {
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
        warn!(interface = %interface, "SocketCAN connection lost — reconnecting in 5s");
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
}
