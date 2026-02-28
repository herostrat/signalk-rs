/// NMEA 2000 output plugin for signalk-rs.
///
/// Subscribes to SignalK paths on the self vessel, converts values to
/// NMEA 2000 PGN messages, and sends them via SocketCAN.
///
/// Config:
/// ```json
/// {
///   "interface": "can0",
///   "source_address": 100,
///   "interval_ms": 1000
/// }
/// ```
use async_trait::async_trait;
use nmea2000::{N2kBus, Pgn, PgnMessage};
use nmea2000_pgns::{
    cog_sog_rapid_update::CogSogRapidUpdate, vessel_heading::VesselHeading,
    wind_data::WindData,
};
use serde::Deserialize;
use signalk_plugin_api::{
    Plugin, PluginContext, PluginError, PluginMetadata, SubscriptionHandle, SubscriptionSpec,
    delta_callback,
};
use signalk_types::{Delta, Subscription};
use std::sync::{Arc, Mutex};
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

// ─── Config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct SendConfig {
    #[serde(default = "default_interface")]
    interface: String,

    #[serde(default = "default_source_address")]
    source_address: u8,

    #[serde(default = "default_interval")]
    interval_ms: u64,
}

fn default_interface() -> String {
    "can0".to_string()
}
fn default_source_address() -> u8 {
    100
}
fn default_interval() -> u64 {
    1000
}

impl Default for SendConfig {
    fn default() -> Self {
        SendConfig {
            interface: default_interface(),
            source_address: default_source_address(),
            interval_ms: default_interval(),
        }
    }
}

// ─── Snapshot ───────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
struct Snapshot {
    heading_true_rad: Option<f64>,
    magnetic_variation_rad: Option<f64>,
    cog_true_rad: Option<f64>,
    sog_mps: Option<f64>,
    wind_speed_apparent_mps: Option<f64>,
    wind_angle_apparent_rad: Option<f64>,
}

impl Snapshot {
    fn update_from_delta(&mut self, delta: &Delta) {
        for update in &delta.updates {
            for pv in &update.values {
                match pv.path.as_str() {
                    "navigation.headingTrue" => {
                        self.heading_true_rad = pv.value.as_f64();
                    }
                    "navigation.magneticVariation" => {
                        self.magnetic_variation_rad = pv.value.as_f64();
                    }
                    "navigation.courseOverGroundTrue" => {
                        self.cog_true_rad = pv.value.as_f64();
                    }
                    "navigation.speedOverGround" => {
                        self.sog_mps = pv.value.as_f64();
                    }
                    "environment.wind.speedApparent" => {
                        self.wind_speed_apparent_mps = pv.value.as_f64();
                    }
                    "environment.wind.angleApparent" => {
                        self.wind_angle_apparent_rad = pv.value.as_f64();
                    }
                    _ => {}
                }
            }
        }
    }
}

// ─── PGN builders ───────────────────────────────────────────────────────────

/// Encoded PGN ready to send.
struct EncodedPgn {
    pgn: u32,
    priority: u8,
    data: Vec<u8>,
}

fn build_pgns(snap: &Snapshot, sid: &mut u8) -> Vec<EncodedPgn> {
    let mut pgns = Vec::new();

    // PGN 127250 — Vessel Heading
    if let Some(heading) = snap.heading_true_rad {
        let mut builder = VesselHeading::builder().sid(*sid as u64).heading(heading);
        // reference_raw: 0 = True, 1 = Magnetic
        builder = builder.reference_raw(0);
        if let Some(var) = snap.magnetic_variation_rad {
            builder = builder.variation(var);
        }
        let msg = builder.build();
        let mut buf = vec![0u8; msg.data_length()];
        if let Ok(len) = msg.encode(&mut buf) {
            buf.truncate(len);
            pgns.push(EncodedPgn {
                pgn: VesselHeading::PGN.as_u32(),
                priority: 2,
                data: buf,
            });
        }
        *sid = sid.wrapping_add(1);
    }

    // PGN 129026 — COG & SOG, Rapid Update
    if snap.cog_true_rad.is_some() || snap.sog_mps.is_some() {
        let mut builder = CogSogRapidUpdate::builder().sid(*sid as u64);
        // cog_reference_raw: 0 = True, 1 = Magnetic
        builder = builder.cog_reference_raw(0);
        if let Some(cog) = snap.cog_true_rad {
            builder = builder.cog(cog);
        }
        if let Some(sog) = snap.sog_mps {
            builder = builder.sog(sog);
        }
        let msg = builder.build();
        let mut buf = vec![0u8; msg.data_length()];
        if let Ok(len) = msg.encode(&mut buf) {
            buf.truncate(len);
            pgns.push(EncodedPgn {
                pgn: CogSogRapidUpdate::PGN.as_u32(),
                priority: 2,
                data: buf,
            });
        }
        *sid = sid.wrapping_add(1);
    }

    // PGN 130306 — Wind Data
    if snap.wind_speed_apparent_mps.is_some() || snap.wind_angle_apparent_rad.is_some() {
        let mut builder = WindData::builder().sid(*sid as u64);
        // reference_raw: 2 = Apparent
        builder = builder.reference_raw(2);
        if let Some(speed) = snap.wind_speed_apparent_mps {
            builder = builder.wind_speed(speed);
        }
        if let Some(angle) = snap.wind_angle_apparent_rad {
            builder = builder.wind_angle(angle);
        }
        let msg = builder.build();
        let mut buf = vec![0u8; msg.data_length()];
        if let Ok(len) = msg.encode(&mut buf) {
            buf.truncate(len);
            pgns.push(EncodedPgn {
                pgn: WindData::PGN.as_u32(),
                priority: 2,
                data: buf,
            });
        }
        *sid = sid.wrapping_add(1);
    }

    pgns
}

// ─── Plugin ─────────────────────────────────────────────────────────────────

pub struct Nmea2000SendPlugin {
    abort_handles: Vec<AbortHandle>,
    subscription: Option<SubscriptionHandle>,
}

impl Nmea2000SendPlugin {
    pub fn new() -> Self {
        Nmea2000SendPlugin {
            abort_handles: Vec::new(),
            subscription: None,
        }
    }
}

impl Default for Nmea2000SendPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for Nmea2000SendPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "nmea2000-send",
            "NMEA 2000 Output",
            "Converts SignalK data to NMEA 2000 PGNs and sends via SocketCAN",
            "0.1.0",
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "interface": {
                    "type": "string",
                    "description": "SocketCAN interface name",
                    "default": "can0"
                },
                "source_address": {
                    "type": "integer",
                    "description": "NMEA 2000 source address (0-252)",
                    "default": 100,
                    "minimum": 0,
                    "maximum": 252
                },
                "interval_ms": {
                    "type": "integer",
                    "description": "PGN send interval in milliseconds",
                    "default": 1000,
                    "minimum": 100
                }
            }
        }))
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let cfg: SendConfig = serde_json::from_value(config)
            .map_err(|e| PluginError::config(format!("invalid nmea2000-send config: {e}")))?;

        info!(
            interface = %cfg.interface,
            source = cfg.source_address,
            interval_ms = cfg.interval_ms,
            "NMEA 2000 output starting"
        );

        let snapshot = Arc::new(Mutex::new(Snapshot::default()));

        // Subscribe to relevant SignalK paths
        let snap_for_sub = snapshot.clone();
        let handle = ctx
            .subscribe(
                SubscriptionSpec::self_vessel(vec![
                    Subscription::path("navigation.headingTrue"),
                    Subscription::path("navigation.magneticVariation"),
                    Subscription::path("navigation.courseOverGroundTrue"),
                    Subscription::path("navigation.speedOverGround"),
                    Subscription::path("environment.wind.speedApparent"),
                    Subscription::path("environment.wind.angleApparent"),
                ]),
                delta_callback(move |delta: Delta| {
                    let mut snap = snap_for_sub.lock().unwrap();
                    snap.update_from_delta(&delta);
                }),
            )
            .await?;
        self.subscription = Some(handle);

        // Channel for sending encoded PGNs to the blocking thread
        let (tx, rx) = std::sync::mpsc::channel::<EncodedPgn>();

        // Blocking thread: owns the N2kBus and sends frames
        let interface = cfg.interface.clone();
        let source_addr = cfg.source_address;
        let bus_handle = tokio::task::spawn_blocking(move || {
            let mut bus = match N2kBus::open(&interface) {
                Ok(b) => b,
                Err(e) => {
                    warn!(error = %e, "Failed to open SocketCAN interface {interface}");
                    return;
                }
            };

            while let Ok(pgn) = rx.recv() {
                if let Err(e) = bus.send_raw(
                    Pgn::new(pgn.pgn),
                    source_addr,
                    pgn.priority,
                    255, // broadcast
                    &pgn.data,
                ) {
                    debug!(pgn = pgn.pgn, error = %e, "Failed to send PGN");
                }
            }
        });
        self.abort_handles.push(bus_handle.abort_handle());

        // Async tick task: builds PGNs and sends via channel
        let interval = tokio::time::Duration::from_millis(cfg.interval_ms);
        let tick_handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            let mut sid: u8 = 0;
            // Skip the first immediate tick
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let snap = snapshot.lock().unwrap().clone();
                let pgns = build_pgns(&snap, &mut sid);
                for pgn in pgns {
                    if tx.send(pgn).is_err() {
                        return; // bus thread exited
                    }
                }
            }
        });
        self.abort_handles.push(tick_handle.abort_handle());

        ctx.set_status(&format!("CAN {}", cfg.interface));
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        self.subscription.take();
        for handle in self.abort_handles.drain(..) {
            handle.abort();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_id() {
        let plugin = Nmea2000SendPlugin::new();
        assert_eq!(plugin.metadata().id, "nmea2000-send");
    }

    #[test]
    fn default_config_deserializes() {
        let config: SendConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(config.interface, "can0");
        assert_eq!(config.source_address, 100);
        assert_eq!(config.interval_ms, 1000);
    }

    #[test]
    fn snapshot_updates_from_delta() {
        let delta: Delta = serde_json::from_value(serde_json::json!({
            "context": "vessels.self",
            "updates": [{
                "source": { "label": "test", "type": "test" },
                "values": [
                    { "path": "navigation.headingTrue", "value": 1.234 },
                    { "path": "navigation.speedOverGround", "value": 5.14 },
                    { "path": "navigation.courseOverGroundTrue", "value": 1.0 },
                    { "path": "environment.wind.angleApparent", "value": 0.786 },
                    { "path": "environment.wind.speedApparent", "value": 10.0 }
                ]
            }]
        }))
        .unwrap();

        let mut snap = Snapshot::default();
        snap.update_from_delta(&delta);

        assert_eq!(snap.heading_true_rad, Some(1.234));
        assert_eq!(snap.sog_mps, Some(5.14));
        assert_eq!(snap.cog_true_rad, Some(1.0));
        assert_eq!(snap.wind_angle_apparent_rad, Some(0.786));
        assert_eq!(snap.wind_speed_apparent_mps, Some(10.0));
    }

    #[test]
    fn build_pgns_from_full_snapshot() {
        let snap = Snapshot {
            heading_true_rad: Some(1.234),
            magnetic_variation_rad: Some(0.05),
            cog_true_rad: Some(1.0),
            sog_mps: Some(5.0),
            wind_speed_apparent_mps: Some(10.0),
            wind_angle_apparent_rad: Some(0.786),
        };

        let mut sid = 0u8;
        let pgns = build_pgns(&snap, &mut sid);

        assert_eq!(pgns.len(), 3);
        assert_eq!(pgns[0].pgn, 127250); // Vessel Heading
        assert_eq!(pgns[1].pgn, 129026); // COG/SOG
        assert_eq!(pgns[2].pgn, 130306); // Wind Data
        assert_eq!(sid, 3); // SID incremented for each
    }

    #[test]
    fn build_pgns_empty_snapshot_produces_nothing() {
        let snap = Snapshot::default();
        let mut sid = 0u8;
        let pgns = build_pgns(&snap, &mut sid);
        assert!(pgns.is_empty());
    }

    #[test]
    fn build_heading_pgn_encodes_correctly() {
        let snap = Snapshot {
            heading_true_rad: Some(std::f64::consts::FRAC_PI_2), // 90 degrees
            ..Default::default()
        };

        let mut sid = 0u8;
        let pgns = build_pgns(&snap, &mut sid);
        assert_eq!(pgns.len(), 1);

        // Decode the encoded data to verify round-trip
        let decoded = VesselHeading::decode(&pgns[0].data).unwrap();
        let heading = decoded.heading().unwrap();
        assert!(
            (heading - std::f64::consts::FRAC_PI_2).abs() < 0.001,
            "Expected ~PI/2, got {heading}"
        );
    }

    #[test]
    fn build_cog_sog_pgn_encodes_correctly() {
        let snap = Snapshot {
            cog_true_rad: Some(1.0),
            sog_mps: Some(5.0),
            ..Default::default()
        };

        let mut sid = 0u8;
        let pgns = build_pgns(&snap, &mut sid);
        assert_eq!(pgns.len(), 1);

        let decoded = CogSogRapidUpdate::decode(&pgns[0].data).unwrap();
        let cog = decoded.cog().unwrap();
        assert!((cog - 1.0).abs() < 0.001, "Expected ~1.0 rad, got {cog}");
        let sog = decoded.sog().unwrap();
        assert!((sog - 5.0).abs() < 0.01, "Expected ~5.0 m/s, got {sog}");
    }

    #[test]
    fn build_wind_pgn_encodes_correctly() {
        let snap = Snapshot {
            wind_speed_apparent_mps: Some(12.0),
            wind_angle_apparent_rad: Some(0.5),
            ..Default::default()
        };

        let mut sid = 0u8;
        let pgns = build_pgns(&snap, &mut sid);
        assert_eq!(pgns.len(), 1);

        let decoded = WindData::decode(&pgns[0].data).unwrap();
        let speed = decoded.wind_speed().unwrap();
        assert!(
            (speed - 12.0).abs() < 0.01,
            "Expected ~12.0 m/s, got {speed}"
        );
        let angle = decoded.wind_angle().unwrap();
        assert!(
            (angle - 0.5).abs() < 0.001,
            "Expected ~0.5 rad, got {angle}"
        );
    }

    #[test]
    fn sid_wraps_around() {
        let snap = Snapshot {
            heading_true_rad: Some(1.0),
            cog_true_rad: Some(1.0),
            wind_speed_apparent_mps: Some(10.0),
            ..Default::default()
        };

        let mut sid = 254u8;
        let pgns = build_pgns(&snap, &mut sid);
        assert_eq!(pgns.len(), 3);
        assert_eq!(sid, 1); // 254 → 255 → 0 → 1
    }
}
