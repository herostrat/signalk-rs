/// NMEA 0183 input plugin for signalk-rs.
///
/// Provides two plugin variants:
/// - [`Nmea0183TcpPlugin`] — listens on a TCP port for NMEA sentences
/// - [`Nmea0183SerialPlugin`] — reads from a serial port device
///
/// Both parse sentences via the shared [`sentences`] module and emit SignalK
/// deltas through the `PluginContext::handle_message` API.
/// AIS VDM/VDO sentences are decoded via the [`ais_decode`] module.
pub mod ais_decode;
pub mod dsc_decode;
pub mod sentences;
pub mod xdr;

use async_trait::async_trait;
use serde::Deserialize;
use signalk_plugin_api::{Plugin, PluginContext, PluginError, PluginMetadata};
use signalk_types::{Delta, PathValue as SkPathValue, Source, Update};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{debug, error, info, warn};

// ─── Config structs ─────────────────────────────────────────────────────────

/// Configuration for the NMEA 0183 TCP input plugin.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[schemars(default)]
pub struct TcpConfig {
    /// Bind address (host:port).
    #[serde(default = "default_tcp_addr")]
    pub addr: String,
    /// Source label for SignalK deltas.
    #[serde(default = "default_tcp_source_label")]
    pub source_label: String,
}

impl Default for TcpConfig {
    fn default() -> Self {
        TcpConfig {
            addr: default_tcp_addr(),
            source_label: default_tcp_source_label(),
        }
    }
}

fn default_tcp_addr() -> String {
    "0.0.0.0:10110".to_string()
}

fn default_tcp_source_label() -> String {
    "nmea0183-tcp".to_string()
}

/// Configuration for the NMEA 0183 serial port input plugin.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SerialConfig {
    /// Serial device path (e.g. /dev/ttyUSB0).
    pub path: String,
    /// Baud rate (standard NMEA: 4800, high-speed mux: 38400).
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    /// Source label for SignalK deltas.
    #[serde(default = "default_serial_source_label")]
    pub source_label: String,
}

fn default_baud_rate() -> u32 {
    4800
}

fn default_serial_source_label() -> String {
    "nmea0183-serial".to_string()
}

// ─── Shared parsing helper ──────────────────────────────────────────────────

/// Parse a raw NMEA sentence string and convert to a SignalK Delta.
/// Returns `None` if the sentence type is unsupported or fails to parse.
pub fn sentence_to_delta(raw: &str, source_label: &str) -> Option<Delta> {
    let parsed = nmea::parse_str(raw)
        .map_err(|e| debug!(sentence = %raw, "NMEA parse error: {e:?}"))
        .ok()?;

    let path_values: Vec<sentences::PathValue> = match parsed {
        nmea::ParseResult::RMC(rmc) => sentences::from_rmc(&rmc),
        nmea::ParseResult::GGA(gga) => sentences::from_gga(&gga),
        nmea::ParseResult::VTG(vtg) => sentences::from_vtg(&vtg),
        nmea::ParseResult::HDT(hdt) => sentences::from_hdt(&hdt),
        nmea::ParseResult::MWV(mwv) => sentences::from_mwv(&mwv),
        nmea::ParseResult::DPT(dpt) => sentences::from_dpt(&dpt),
        nmea::ParseResult::HDG(hdg) => sentences::from_hdg(&hdg),
        nmea::ParseResult::HDM(hdm) => sentences::from_hdm(&hdm),
        nmea::ParseResult::VHW(vhw) => sentences::from_vhw(&vhw),
        nmea::ParseResult::MTW(mtw) => sentences::from_mtw(&mtw),
        nmea::ParseResult::ROT(rot) => sentences::from_rot(&rot),
        nmea::ParseResult::MDA(mda) => sentences::from_mda(&mda),
        nmea::ParseResult::MWD(mwd) => sentences::from_mwd(&mwd),
        nmea::ParseResult::RSA(rsa) => sentences::from_rsa(&rsa),
        nmea::ParseResult::RPM(rpm) => sentences::from_rpm(&rpm),
        nmea::ParseResult::GLL(gll) => sentences::from_gll(&gll),
        nmea::ParseResult::DBT(dbt) => sentences::from_dbt(&dbt),
        nmea::ParseResult::DBS(dbs) => sentences::from_dbs(&dbs),
        nmea::ParseResult::DBK(dbk) => sentences::from_dbk(&dbk),
        nmea::ParseResult::VDR(vdr) => sentences::from_vdr(&vdr),
        nmea::ParseResult::VLW(vlw) => sentences::from_vlw(&vlw),
        nmea::ParseResult::VWR(vwr) => sentences::from_vwr(&vwr),
        nmea::ParseResult::VWT(vwt) => sentences::from_vwt(&vwt),
        nmea::ParseResult::RMB(rmb) => sentences::from_rmb(&rmb),
        nmea::ParseResult::BWC(bwc) => sentences::from_bwc(&bwc),
        nmea::ParseResult::BOD(bod) => sentences::from_bod(&bod),
        nmea::ParseResult::XTE(xte) => sentences::from_xte(&xte),
        nmea::ParseResult::GSA(gsa) => sentences::from_gsa(&gsa),
        nmea::ParseResult::GNS(gns) => sentences::from_gns(&gns),
        nmea::ParseResult::ZDA(zda) => sentences::from_zda(&zda),
        nmea::ParseResult::XDR(ref xdr) => xdr::from_xdr(xdr),
        _ => return None,
    };

    if path_values.is_empty() {
        return None;
    }

    let sk_values: Vec<SkPathValue> = path_values
        .into_iter()
        .map(|pv| SkPathValue::new(pv.path, pv.value))
        .collect();

    let talker = raw
        .strip_prefix('$')
        .and_then(|s| s.get(..2))
        .unwrap_or("UN");

    let source = Source::nmea0183(source_label, talker);
    let update = Update::new(source, sk_values);
    Some(Delta::self_vessel(vec![update]))
}

// ─── TCP Plugin ─────────────────────────────────────────────────────────────

/// NMEA 0183 TCP input plugin.
///
/// Listens on a TCP port for NMEA sentence streams (e.g. from a multiplexer
/// like Yacht Devices YDWG-02 or an OpenCPN NMEA server).
///
/// Config:
/// ```json
/// { "addr": "0.0.0.0:10110", "source_label": "gps" }
/// ```
pub struct Nmea0183TcpPlugin {
    abort_handle: Option<tokio::task::AbortHandle>,
}

impl Nmea0183TcpPlugin {
    pub fn new() -> Self {
        Nmea0183TcpPlugin { abort_handle: None }
    }
}

impl Default for Nmea0183TcpPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for Nmea0183TcpPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "nmea0183-tcp",
            "NMEA 0183 TCP",
            "TCP input for NMEA 0183 sentences",
            "0.1.0",
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::to_value(schemars::schema_for!(TcpConfig)).unwrap())
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let cfg: TcpConfig =
            serde_json::from_value(config).map_err(|e| PluginError::config(format!("{e}")))?;

        let addr: SocketAddr = cfg
            .addr
            .parse()
            .map_err(|e| PluginError::config(format!("invalid addr: {e}")))?;

        let source_label = cfg.source_label;

        let handle = tokio::spawn(async move {
            if let Err(e) = run_tcp_listener(addr, &source_label, ctx).await {
                error!("NMEA TCP plugin error: {e}");
            }
        })
        .abort_handle();

        self.abort_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(h) = self.abort_handle.take() {
            h.abort();
        }
        Ok(())
    }
}

async fn run_tcp_listener(
    addr: SocketAddr,
    source_label: &str,
    ctx: Arc<dyn PluginContext>,
) -> Result<(), PluginError> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(PluginError::Io)?;

    ctx.set_status(&format!("Listening on {addr}"));
    info!(addr = %addr, "NMEA TCP plugin listening");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                info!(%peer, "NMEA TCP connection accepted");
                let ctx = ctx.clone();
                let label = source_label.to_string();
                tokio::spawn(async move {
                    if let Err(e) = handle_tcp_connection(stream, peer, &label, ctx).await {
                        warn!(%peer, "NMEA TCP connection closed: {e}");
                    }
                });
            }
            Err(e) => {
                error!("NMEA TCP accept error: {e}");
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            }
        }
    }
}

async fn handle_tcp_connection(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    label: &str,
    ctx: Arc<dyn PluginContext>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();
    let mut ais_decoder = ais_decode::AisDecoder::new(label);

    while let Some(line) = lines.next_line().await? {
        let sentence = line.trim().to_string();
        if sentence.is_empty() {
            continue;
        }
        debug!(%peer, %sentence, "NMEA sentence");

        // AIS first (stateful — fragment reassembly)
        if let Some(delta) = ais_decoder.try_decode(&sentence) {
            ctx.handle_message(delta).await.ok();
            continue;
        }
        // DSC (stateless — produces other-vessel deltas + optional notification)
        if let Some(deltas) = dsc_decode::try_decode_dsc(&sentence, label) {
            for delta in deltas {
                ctx.handle_message(delta).await.ok();
            }
            continue;
        }
        // Standard NMEA (stateless — self-vessel)
        if let Some(delta) = sentence_to_delta(&sentence, label) {
            ctx.handle_message(delta).await.ok();
        }
    }
    Ok(())
}

// ─── Serial Plugin ──────────────────────────────────────────────────────────

/// NMEA 0183 serial port input plugin.
///
/// Opens a serial device (e.g. `/dev/ttyUSB0`) and reads NMEA sentences.
/// On read errors, retries after a 5 second backoff.
///
/// Config:
/// ```json
/// { "path": "/dev/ttyUSB0", "baud_rate": 4800, "source_label": "gps" }
/// ```
pub struct Nmea0183SerialPlugin {
    abort_handle: Option<tokio::task::AbortHandle>,
}

impl Nmea0183SerialPlugin {
    pub fn new() -> Self {
        Nmea0183SerialPlugin { abort_handle: None }
    }
}

impl Default for Nmea0183SerialPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for Nmea0183SerialPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "nmea0183-serial",
            "NMEA 0183 Serial",
            "Serial port input for NMEA 0183 sentences",
            "0.1.0",
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::to_value(schemars::schema_for!(SerialConfig)).unwrap())
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let cfg: SerialConfig =
            serde_json::from_value(config).map_err(|e| PluginError::config(format!("{e}")))?;

        let path = cfg.path;
        let baud_rate = cfg.baud_rate;
        let source_label = cfg.source_label;

        let handle = tokio::spawn(async move {
            if let Err(e) = run_serial_reader(&path, baud_rate, &source_label, ctx).await {
                error!(path = %path, "NMEA serial plugin error: {e}");
            }
        })
        .abort_handle();

        self.abort_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        if let Some(h) = self.abort_handle.take() {
            h.abort();
        }
        Ok(())
    }
}

async fn run_serial_reader(
    path: &str,
    baud_rate: u32,
    source_label: &str,
    ctx: Arc<dyn PluginContext>,
) -> Result<(), PluginError> {
    ctx.set_status(&format!("Opening {path} at {baud_rate} baud"));
    info!(path = %path, baud_rate, "NMEA serial plugin starting");

    loop {
        match open_and_read_serial(path, baud_rate, source_label, &ctx).await {
            Ok(()) => break,
            Err(e) => {
                ctx.set_error(&format!("Serial error: {e}"));
                error!(path = %path, "Serial error: {e} — retrying in 5 s");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    }
    Ok(())
}

async fn open_and_read_serial(
    path: &str,
    baud_rate: u32,
    source_label: &str,
    ctx: &Arc<dyn PluginContext>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio_serial::SerialPortBuilderExt;

    let port = tokio_serial::new(path, baud_rate).open_native_async()?;
    ctx.set_status(&format!("Connected to {path}"));
    info!(path = %path, "Serial port opened");

    let mut lines = BufReader::new(port).lines();
    let mut ais_decoder = ais_decode::AisDecoder::new(source_label);

    while let Some(line) = lines.next_line().await? {
        let sentence = line.trim().to_string();
        if sentence.is_empty() {
            continue;
        }
        debug!(sentence = %sentence, "NMEA serial sentence");

        // AIS first (stateful — fragment reassembly)
        if let Some(delta) = ais_decoder.try_decode(&sentence) {
            ctx.handle_message(delta).await.ok();
            continue;
        }
        // DSC (stateless — produces other-vessel deltas + optional notification)
        if let Some(deltas) = dsc_decode::try_decode_dsc(&sentence, source_label) {
            for delta in deltas {
                ctx.handle_message(delta).await.ok();
            }
            continue;
        }
        // Standard NMEA (stateless — self-vessel)
        if let Some(delta) = sentence_to_delta(&sentence, source_label) {
            ctx.handle_message(delta).await.ok();
        }
    }

    warn!(path = %path, "Serial port EOF / closed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_to_delta_rmc() {
        let raw = "$GPRMC,225446.33,A,4916.45,N,12311.12,W,000.5,054.7,191194,020.3,E,A*2B";
        let delta = sentence_to_delta(raw, "gps");
        assert!(delta.is_some());
        let delta = delta.unwrap();
        let update = &delta.updates[0];
        assert!(
            update
                .values
                .iter()
                .any(|v| v.path == "navigation.position")
        );
        assert!(
            update
                .values
                .iter()
                .any(|v| v.path == "navigation.speedOverGround")
        );
    }

    #[test]
    fn sentence_to_delta_source_label() {
        let raw = "$GPRMC,225446.33,A,4916.45,N,12311.12,W,000.5,054.7,191194,020.3,E,A*2B";
        let delta = sentence_to_delta(raw, "my-gps").unwrap();
        assert_eq!(delta.updates[0].source.label, "my-gps");
    }

    #[test]
    fn sentence_to_delta_talker_extracted() {
        let raw = "$IIRMC,225446.33,A,4916.45,N,12311.12,W,000.5,054.7,191194,020.3,E,A*3C";
        if let Some(d) = sentence_to_delta(raw, "mux") {
            let extra = &d.updates[0].source.extra;
            assert_eq!(extra.get("talker"), Some(&serde_json::json!("II")));
        }
    }

    #[test]
    fn sentence_to_delta_unsupported() {
        // GSV (Satellites in View) is not in our dispatch table
        let raw = "$GPGSV,3,1,11,03,03,111,00,04,15,270,00,06,01,010,00,13,06,292,00*74";
        assert!(sentence_to_delta(raw, "gps").is_none());
    }

    #[test]
    fn sentence_to_delta_invalid() {
        assert!(sentence_to_delta("not a sentence", "gps").is_none());
    }

    #[tokio::test]
    async fn tcp_plugin_metadata() {
        let plugin = Nmea0183TcpPlugin::new();
        let meta = plugin.metadata();
        assert_eq!(meta.id, "nmea0183-tcp");
    }

    #[tokio::test]
    async fn serial_plugin_metadata() {
        let plugin = Nmea0183SerialPlugin::new();
        let meta = plugin.metadata();
        assert_eq!(meta.id, "nmea0183-serial");
    }

    #[tokio::test]
    async fn tcp_plugin_start_stop() {
        use signalk_plugin_api::testing::MockPluginContext;

        let mut plugin = Nmea0183TcpPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        // Start on a random port
        let result = plugin
            .start(
                serde_json::json!({"addr": "127.0.0.1:0", "source_label": "test"}),
                ctx,
            )
            .await;
        assert!(result.is_ok());

        // Stop should work
        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn tcp_plugin_rejects_invalid_addr() {
        use signalk_plugin_api::testing::MockPluginContext;

        let mut plugin = Nmea0183TcpPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());
        let result = plugin
            .start(serde_json::json!({"addr": "not-a-socket-addr"}), ctx)
            .await;
        assert!(result.is_err());
    }
}
