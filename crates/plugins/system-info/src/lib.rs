//! System information plugin for signalk-rs.
//!
//! Exposes OS, network, and hardware metrics via REST endpoints in the plugin
//! namespace. Also writes time paths to the SK store every 30 seconds as a
//! fallback source — only when NTP is synchronised. This allows `environment.time.*`
//! to have a value even when no GPS is connected.
//!
//! GPS and other higher-frequency sources naturally dominate because they update
//! more often (last-write-wins at equal priority). Configure `[source_ttls]` in
//! `signalk-rs.toml` so GPS values expire after a short TTL, letting system-info
//! take over when GPS goes offline.
//!
//! # Endpoints
//! - `GET /plugins/system-info/time`    — system clock, NTP status, timezone
//! - `GET /plugins/system-info/network` — marine network interfaces (CAN, WiFi, Ethernet)
//! - `GET /plugins/system-info/system`  — CPU load, RAM, disk, uptime, temperature
//!
//! # Config example (signalk-rs.toml)
//! ```toml
//! [[plugins]]
//! id = "system-info"
//! enabled = true
//!
//! # Optional: evict GPS time after 5 s of silence so system-info fills in
//! [source_ttls]
//! "nmea0183-tcp.GP" = 5
//! "nmea2000.129033" = 5
//! ```
pub mod network;
pub mod syshealth;
pub mod time_sync;

use async_trait::async_trait;
use signalk_plugin_api::{
    Plugin, PluginContext, PluginError, PluginMetadata, RouterSetup, route_handler,
};
use signalk_types::{Delta, PathValue, Source, Update};
use std::sync::Arc;

// ─── Plugin struct ───────────────────────────────────────────────────────────

pub struct SystemInfoPlugin {
    ticker_handle: Option<tokio::task::AbortHandle>,
}

impl SystemInfoPlugin {
    pub fn new() -> Self {
        SystemInfoPlugin {
            ticker_handle: None,
        }
    }
}

impl Default for SystemInfoPlugin {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Plugin trait impl ───────────────────────────────────────────────────────

#[async_trait]
impl Plugin for SystemInfoPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "system-info",
            "System Info",
            "Exposes OS/network/hardware metrics via REST; NTP fallback time to SK store",
            "0.1.0",
        )
    }

    async fn start(
        &mut self,
        _config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        ctx.set_status("Running");

        // ── Background ticker: NTP fallback time → SK store ──────────────────
        // Writes navigation.datetime + environment.time.* every 30 s when NTP is
        // synchronised. GPS dominates via frequency (1 Hz vs 0.033 Hz). With
        // [source_ttls] configured, GPS values expire and system-info fills in.
        let ctx_tick = Arc::clone(&ctx);
        let handle = tokio::spawn(async move {
            loop {
                if time_sync::ntp_synchronized() {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let tz = time_sync::local_timezone();
                    let iso = chrono::Utc::now()
                        .format("%Y-%m-%dT%H:%M:%S.000Z")
                        .to_string();

                    let delta = Delta::self_vessel(vec![Update::new(
                        Source::plugin("system-info"),
                        vec![
                            PathValue::new("navigation.datetime", serde_json::json!(iso)),
                            PathValue::new("environment.time.millis", serde_json::json!(now_ms)),
                            PathValue::new(
                                "environment.time.timezoneOffset",
                                serde_json::json!(0i64),
                            ),
                            PathValue::new(
                                "environment.time.timezoneRegion",
                                serde_json::json!(tz),
                            ),
                        ],
                    )]);
                    ctx_tick.handle_message(delta).await.ok();
                }
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        })
        .abort_handle();
        self.ticker_handle = Some(handle);

        // ── REST routes (on-demand reads) ─────────────────────────────────────
        ctx.register_routes(
            Box::new(move |router: &mut dyn signalk_plugin_api::PluginRouter| {
                // GET /plugins/system-info/time
                router.get(
                    "/time",
                    route_handler(|_req| async move {
                        let status = time_sync::time_status();
                        signalk_plugin_api::PluginResponse::json(
                            200,
                            &time_sync::time_status_json(&status),
                        )
                    }),
                );

                // GET /plugins/system-info/network
                router.get(
                    "/network",
                    route_handler(|_req| async move {
                        let ifaces = network::list_interfaces();
                        signalk_plugin_api::PluginResponse::json(
                            200,
                            &network::interfaces_json(&ifaces),
                        )
                    }),
                );

                // GET /plugins/system-info/system
                router.get(
                    "/system",
                    route_handler(|_req| async move {
                        let health = syshealth::sys_health();
                        signalk_plugin_api::PluginResponse::json(
                            200,
                            &syshealth::sys_health_json(&health),
                        )
                    }),
                );
            }) as RouterSetup,
        )
        .await?;

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(h) = self.ticker_handle.take() {
            h.abort();
        }
        Ok(())
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_metadata() {
        let plugin = SystemInfoPlugin::new();
        let meta = plugin.metadata();
        assert_eq!(meta.id, "system-info");
        assert_eq!(meta.name, "System Info");
    }

    #[tokio::test]
    async fn plugin_start_stop() {
        use signalk_plugin_api::testing::MockPluginContext;

        let mut plugin = SystemInfoPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());
        let result = plugin.start(serde_json::json!({}), ctx).await;
        assert!(result.is_ok());
        plugin.stop().await.unwrap();
        // ticker_handle is aborted — plugin is fully stopped
        assert!(plugin.ticker_handle.is_none());
    }
}
