/// NMEA 0183 output plugin for signalk-rs.
///
/// Subscribes to SignalK paths on the self vessel, converts values back to
/// NMEA 0183 sentences, and broadcasts them to connected TCP clients.
///
/// Config:
/// ```json
/// {
///   "port": 10111,
///   "talker_id": "GP",
///   "interval_ms": 1000
/// }
/// ```
use async_trait::async_trait;
use nmea::generate::generate_sentence;
use nmea::sentences::{
    dbt::DbtData, hdt::HdtData, mwv::MwvData, mwv::MwvReference, mwv::MwvWindSpeedUnits,
    rmc::RmcData, rmc::RmcStatusOfFix, vtg::VtgData,
};
use serde::Deserialize;
use signalk_plugin_api::{
    Plugin, PluginContext, PluginError, PluginMetadata, SubscriptionHandle, SubscriptionSpec,
    delta_callback,
};
use signalk_types::{Delta, Subscription};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

// ─── Config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct SendConfig {
    #[serde(default = "default_port")]
    port: u16,

    #[serde(default = "default_talker_id")]
    talker_id: String,

    #[serde(default = "default_interval")]
    interval_ms: u64,
}

fn default_port() -> u16 {
    10111
}
fn default_talker_id() -> String {
    "GP".to_string()
}
fn default_interval() -> u64 {
    1000
}

impl Default for SendConfig {
    fn default() -> Self {
        SendConfig {
            port: default_port(),
            talker_id: default_talker_id(),
            interval_ms: default_interval(),
        }
    }
}

// ─── Snapshot ───────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
struct Snapshot {
    latitude: Option<f64>,
    longitude: Option<f64>,
    sog_mps: Option<f64>,
    cog_true_rad: Option<f64>,
    heading_true_rad: Option<f64>,
    magnetic_variation_rad: Option<f64>,
    wind_angle_apparent_rad: Option<f64>,
    wind_speed_apparent_mps: Option<f64>,
    depth_below_transducer_m: Option<f64>,
}

impl Snapshot {
    fn update_from_delta(&mut self, delta: &Delta) {
        for update in &delta.updates {
            for pv in &update.values {
                match pv.path.as_str() {
                    "navigation.position" => {
                        self.latitude = pv.value.get("latitude").and_then(|v| v.as_f64());
                        self.longitude = pv.value.get("longitude").and_then(|v| v.as_f64());
                    }
                    "navigation.speedOverGround" => {
                        self.sog_mps = pv.value.as_f64();
                    }
                    "navigation.courseOverGroundTrue" => {
                        self.cog_true_rad = pv.value.as_f64();
                    }
                    "navigation.headingTrue" => {
                        self.heading_true_rad = pv.value.as_f64();
                    }
                    "navigation.magneticVariation" => {
                        self.magnetic_variation_rad = pv.value.as_f64();
                    }
                    "environment.wind.angleApparent" => {
                        self.wind_angle_apparent_rad = pv.value.as_f64();
                    }
                    "environment.wind.speedApparent" => {
                        self.wind_speed_apparent_mps = pv.value.as_f64();
                    }
                    "environment.depth.belowTransducer" => {
                        self.depth_below_transducer_m = pv.value.as_f64();
                    }
                    _ => {}
                }
            }
        }
    }
}

// ─── Conversions ────────────────────────────────────────────────────────────

const MPS_TO_KNOTS: f64 = 1.943_844;
const FEET_PER_METER: f64 = 3.280_84;
const FATHOMS_PER_METER: f64 = 0.546_807;

fn snapshot_to_rmc(snap: &Snapshot) -> RmcData {
    let now = chrono::Utc::now().naive_utc();
    RmcData {
        fix_time: Some(now.time()),
        fix_date: Some(now.date()),
        status_of_fix: if snap.latitude.is_some() {
            RmcStatusOfFix::Autonomous
        } else {
            RmcStatusOfFix::Invalid
        },
        lat: snap.latitude,
        lon: snap.longitude,
        speed_over_ground: snap.sog_mps.map(|v| (v * MPS_TO_KNOTS) as f32),
        true_course: snap.cog_true_rad.map(|v| v.to_degrees() as f32),
        magnetic_variation: snap.magnetic_variation_rad.map(|v| v.to_degrees() as f32),
        faa_mode: None,
        nav_status: None,
    }
}

fn snapshot_to_hdt(snap: &Snapshot) -> HdtData {
    HdtData {
        heading: snap.heading_true_rad.map(|v| v.to_degrees() as f32),
    }
}

fn snapshot_to_mwv(snap: &Snapshot) -> MwvData {
    MwvData {
        wind_direction: snap.wind_angle_apparent_rad.map(|v| v.to_degrees() as f32),
        reference: Some(MwvReference::Relative),
        wind_speed: snap
            .wind_speed_apparent_mps
            .map(|v| (v * MPS_TO_KNOTS) as f32),
        wind_speed_units: Some(MwvWindSpeedUnits::Knots),
        data_valid: snap.wind_angle_apparent_rad.is_some(),
    }
}

fn snapshot_to_dbt(snap: &Snapshot) -> DbtData {
    DbtData {
        depth_feet: snap.depth_below_transducer_m.map(|v| v * FEET_PER_METER),
        depth_meters: snap.depth_below_transducer_m,
        depth_fathoms: snap.depth_below_transducer_m.map(|v| v * FATHOMS_PER_METER),
    }
}

fn snapshot_to_vtg(snap: &Snapshot) -> VtgData {
    VtgData {
        true_course: snap.cog_true_rad.map(|v| v.to_degrees() as f32),
        speed_over_ground: snap.sog_mps.map(|v| (v * MPS_TO_KNOTS) as f32),
    }
}

fn generate_all_sentences(snap: &Snapshot, talker_id: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut buf = String::with_capacity(128);

    // RMC — always generate (contains position, SOG, COG, time)
    let rmc = snapshot_to_rmc(snap);
    buf.clear();
    if generate_sentence(talker_id, &rmc, &mut buf).is_ok() {
        sentences.push(format!("{buf}\r\n"));
    }

    // HDT — only when heading available
    if snap.heading_true_rad.is_some() {
        let hdt = snapshot_to_hdt(snap);
        buf.clear();
        if generate_sentence(talker_id, &hdt, &mut buf).is_ok() {
            sentences.push(format!("{buf}\r\n"));
        }
    }

    // MWV — only when wind data available
    if snap.wind_angle_apparent_rad.is_some() || snap.wind_speed_apparent_mps.is_some() {
        let mwv = snapshot_to_mwv(snap);
        buf.clear();
        if generate_sentence(talker_id, &mwv, &mut buf).is_ok() {
            sentences.push(format!("{buf}\r\n"));
        }
    }

    // DBT — only when depth available
    if snap.depth_below_transducer_m.is_some() {
        let dbt = snapshot_to_dbt(snap);
        buf.clear();
        if generate_sentence(talker_id, &dbt, &mut buf).is_ok() {
            sentences.push(format!("{buf}\r\n"));
        }
    }

    // VTG — only when COG/SOG available
    if snap.cog_true_rad.is_some() || snap.sog_mps.is_some() {
        let vtg = snapshot_to_vtg(snap);
        buf.clear();
        if generate_sentence(talker_id, &vtg, &mut buf).is_ok() {
            sentences.push(format!("{buf}\r\n"));
        }
    }

    sentences
}

// ─── Plugin ─────────────────────────────────────────────────────────────────

pub struct Nmea0183SendPlugin {
    abort_handles: Vec<AbortHandle>,
    subscription: Option<SubscriptionHandle>,
}

impl Nmea0183SendPlugin {
    pub fn new() -> Self {
        Nmea0183SendPlugin {
            abort_handles: Vec::new(),
            subscription: None,
        }
    }
}

impl Default for Nmea0183SendPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for Nmea0183SendPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "nmea0183-send",
            "NMEA 0183 Output",
            "Converts SignalK data to NMEA 0183 sentences and serves via TCP",
            "0.1.0",
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "port": {
                    "type": "integer",
                    "description": "TCP port for NMEA sentence output",
                    "default": 10111,
                    "minimum": 1024
                },
                "talker_id": {
                    "type": "string",
                    "description": "NMEA talker ID (e.g. GP, GN, II)",
                    "default": "GP"
                },
                "interval_ms": {
                    "type": "integer",
                    "description": "Sentence generation interval in milliseconds",
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
            .map_err(|e| PluginError::config(format!("invalid nmea0183-send config: {e}")))?;

        info!(
            port = cfg.port,
            talker_id = %cfg.talker_id,
            interval_ms = cfg.interval_ms,
            "NMEA 0183 output starting"
        );

        let snapshot = Arc::new(Mutex::new(Snapshot::default()));

        // Subscribe to relevant SignalK paths
        let snap_for_sub = snapshot.clone();
        let handle = ctx
            .subscribe(
                SubscriptionSpec::self_vessel(vec![
                    Subscription::path("navigation.position"),
                    Subscription::path("navigation.speedOverGround"),
                    Subscription::path("navigation.courseOverGroundTrue"),
                    Subscription::path("navigation.headingTrue"),
                    Subscription::path("navigation.magneticVariation"),
                    Subscription::path("environment.wind.angleApparent"),
                    Subscription::path("environment.wind.speedApparent"),
                    Subscription::path("environment.depth.belowTransducer"),
                ]),
                delta_callback(move |delta: Delta| {
                    let mut snap = snap_for_sub.lock().unwrap();
                    snap.update_from_delta(&delta);
                }),
            )
            .await?;
        self.subscription = Some(handle);

        // Broadcast channel for sentences → TCP clients
        let (tx, _) = broadcast::channel::<String>(64);

        // TCP listener task
        let tx_for_listener = tx.clone();
        let addr = format!("0.0.0.0:{}", cfg.port);
        let listener = TcpListener::bind(&addr).await.map_err(|e| {
            PluginError::runtime(format!("failed to bind NMEA TCP output on {addr}: {e}"))
        })?;

        let listener_handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((mut stream, peer)) => {
                        info!(%peer, "NMEA output client connected");
                        let mut rx = tx_for_listener.subscribe();
                        tokio::spawn(async move {
                            loop {
                                match rx.recv().await {
                                    Ok(sentence) => {
                                        if stream.write_all(sentence.as_bytes()).await.is_err() {
                                            debug!(%peer, "NMEA output client disconnected");
                                            return;
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(n)) => {
                                        debug!(%peer, skipped = n, "client lagged, skipping");
                                    }
                                    Err(broadcast::error::RecvError::Closed) => return,
                                }
                            }
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "TCP accept error");
                    }
                }
            }
        });
        self.abort_handles.push(listener_handle.abort_handle());

        // Sentence generation tick task
        let interval = tokio::time::Duration::from_millis(cfg.interval_ms);
        let talker_id = cfg.talker_id;
        let tick_handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Skip the first immediate tick — no data yet
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let snap = snapshot.lock().unwrap().clone();
                let sentences = generate_all_sentences(&snap, &talker_id);
                for s in sentences {
                    // Ignore send errors (no subscribers = no clients connected)
                    let _ = tx.send(s);
                }
            }
        });
        self.abort_handles.push(tick_handle.abort_handle());

        ctx.set_status(&format!("TCP :{}", cfg.port));
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        // Drop subscription handle to unsubscribe
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
        let plugin = Nmea0183SendPlugin::new();
        assert_eq!(plugin.metadata().id, "nmea0183-send");
    }

    #[test]
    fn default_config_deserializes() {
        let config: SendConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(config.port, 10111);
        assert_eq!(config.talker_id, "GP");
        assert_eq!(config.interval_ms, 1000);
    }

    #[test]
    fn snapshot_updates_from_delta() {
        let delta: Delta = serde_json::from_value(serde_json::json!({
            "context": "vessels.self",
            "updates": [{
                "source": { "label": "test", "type": "test" },
                "values": [
                    { "path": "navigation.position", "value": { "latitude": 54.5, "longitude": 10.0 } },
                    { "path": "navigation.speedOverGround", "value": 5.14 },
                    { "path": "navigation.courseOverGroundTrue", "value": 1.234 },
                    { "path": "navigation.headingTrue", "value": 1.0 },
                    { "path": "environment.depth.belowTransducer", "value": 12.5 }
                ]
            }]
        }))
        .unwrap();

        let mut snap = Snapshot::default();
        snap.update_from_delta(&delta);

        assert_eq!(snap.latitude, Some(54.5));
        assert_eq!(snap.longitude, Some(10.0));
        assert_eq!(snap.sog_mps, Some(5.14));
        assert_eq!(snap.cog_true_rad, Some(1.234));
        assert_eq!(snap.heading_true_rad, Some(1.0));
        assert_eq!(snap.depth_below_transducer_m, Some(12.5));
    }

    #[test]
    fn generate_rmc_sentence() {
        let snap = Snapshot {
            latitude: Some(54.5),
            longitude: Some(10.0),
            sog_mps: Some(5.14),
            cog_true_rad: Some(1.234),
            ..Default::default()
        };

        let sentences = generate_all_sentences(&snap, "GP");
        let rmc = sentences.iter().find(|s| s.contains("RMC")).unwrap();
        assert!(rmc.starts_with("$GPRMC,"));
        assert!(rmc.ends_with("\r\n"));
        assert!(rmc.contains("*")); // checksum present
    }

    #[test]
    fn generate_hdt_sentence() {
        let snap = Snapshot {
            heading_true_rad: Some(std::f64::consts::PI),
            ..Default::default()
        };

        let sentences = generate_all_sentences(&snap, "GP");
        let hdt = sentences.iter().find(|s| s.contains("HDT")).unwrap();
        assert!(hdt.starts_with("$GPHDT,"));
        // PI radians = 180 degrees
        assert!(hdt.contains("180"));
    }

    #[test]
    fn generate_mwv_sentence() {
        let snap = Snapshot {
            wind_angle_apparent_rad: Some(0.786), // ~45 degrees
            wind_speed_apparent_mps: Some(10.0),
            ..Default::default()
        };

        let sentences = generate_all_sentences(&snap, "II");
        let mwv = sentences.iter().find(|s| s.contains("MWV")).unwrap();
        assert!(mwv.starts_with("$IIMWV,"));
        assert!(mwv.contains(",R,")); // Relative reference
        assert!(mwv.contains(",N,")); // Knots
    }

    #[test]
    fn generate_dbt_sentence() {
        let snap = Snapshot {
            depth_below_transducer_m: Some(15.0),
            ..Default::default()
        };

        let sentences = generate_all_sentences(&snap, "SD");
        let dbt = sentences.iter().find(|s| s.contains("DBT")).unwrap();
        assert!(dbt.starts_with("$SDDBT,"));
        assert!(dbt.contains("15")); // meters value
    }

    #[test]
    fn generate_vtg_sentence() {
        let snap = Snapshot {
            cog_true_rad: Some(0.0),
            sog_mps: Some(5.14),
            ..Default::default()
        };

        let sentences = generate_all_sentences(&snap, "GP");
        let vtg = sentences.iter().find(|s| s.contains("VTG")).unwrap();
        assert!(vtg.starts_with("$GPVTG,"));
    }

    #[test]
    fn no_sentences_for_empty_snapshot() {
        let snap = Snapshot::default();
        let sentences = generate_all_sentences(&snap, "GP");
        // Only RMC is always generated (with invalid status)
        assert_eq!(sentences.len(), 1);
        let rmc = &sentences[0];
        assert!(rmc.contains("RMC"));
    }

    #[test]
    fn full_snapshot_generates_all_sentences() {
        let snap = Snapshot {
            latitude: Some(54.5),
            longitude: Some(10.0),
            sog_mps: Some(5.0),
            cog_true_rad: Some(1.0),
            heading_true_rad: Some(1.0),
            magnetic_variation_rad: Some(0.05),
            wind_angle_apparent_rad: Some(0.5),
            wind_speed_apparent_mps: Some(8.0),
            depth_below_transducer_m: Some(20.0),
        };

        let sentences = generate_all_sentences(&snap, "GP");
        assert_eq!(sentences.len(), 5); // RMC, HDT, MWV, DBT, VTG

        let types: Vec<&str> = sentences.iter().map(|s| &s[3..6]).collect();
        assert!(types.contains(&"RMC"));
        assert!(types.contains(&"HDT"));
        assert!(types.contains(&"MWV"));
        assert!(types.contains(&"DBT"));
        assert!(types.contains(&"VTG"));
    }

    #[tokio::test]
    async fn start_and_stop() {
        use signalk_plugin_api::testing::MockPluginContext;

        let mut plugin = Nmea0183SendPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        let result = plugin.start(serde_json::json!({"port": 19283}), ctx).await;
        assert!(result.is_ok());

        plugin.stop().await.unwrap();
    }
}
