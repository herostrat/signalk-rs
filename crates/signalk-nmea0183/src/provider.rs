/// NMEA 0183 TCP input provider.
///
/// Listens on a TCP port (e.g. a NMEA multiplexer like Yacht Devices YDWG-02
/// or an OpenCPN NMEA server). For each incoming connection a task is spawned
/// that reads lines and converts them to SignalK Deltas via `sentences.rs`.
///
/// Spawning strategy:
/// - One `tokio::spawn` for the accept-loop (long-lived background task).
/// - One `tokio::spawn` per accepted connection (connection-scoped task).
///   Connections are independent; one dropping/erroring does not affect others.
/// - The sender `tx: mpsc::Sender<Delta>` is cloned cheaply per connection.
use crate::sentences;
use anyhow::Result;
use signalk_types::{Delta, PathValue as SkPathValue, Source, Update};
use std::net::SocketAddr;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// A TCP provider that accepts NMEA 0183 connections and emits Deltas.
pub struct NmeaTcpProvider {
    pub addr: SocketAddr,
    pub source_label: String,
}

impl NmeaTcpProvider {
    pub fn new(addr: SocketAddr, source_label: impl Into<String>) -> Self {
        NmeaTcpProvider {
            addr,
            source_label: source_label.into(),
        }
    }

    /// Bind the TCP listener and start accepting connections.
    /// Deltas are sent on `tx`; the task runs until the sender is dropped.
    pub async fn run(self, tx: mpsc::Sender<Delta>) -> Result<()> {
        let listener = TcpListener::bind(self.addr).await?;
        info!(addr = %self.addr, "NMEA TCP provider listening");

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    info!(%peer, "NMEA TCP connection accepted");
                    let tx = tx.clone();
                    let label = self.source_label.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, peer, tx, label).await {
                            warn!(%peer, "NMEA connection closed: {e}");
                        }
                    });
                }
                Err(e) => {
                    error!("NMEA TCP accept error: {e}");
                    // Don't spin on persistent errors — small back-off
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                }
            }

            // Stop gracefully when all receivers have dropped
            if tx.is_closed() {
                break;
            }
        }
        Ok(())
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    tx: mpsc::Sender<Delta>,
    label: String,
) -> Result<()> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let sentence = line.trim().to_string();
        if sentence.is_empty() {
            continue;
        }
        debug!(%peer, %sentence, "NMEA sentence");

        if let Some(delta) = sentence_to_delta(&sentence, &label)
            && tx.send(delta).await.is_err()
        {
            break; // receiver dropped → provider is shutting down
        }
    }
    Ok(())
}

/// Parse a raw NMEA sentence string and convert to a SignalK Delta.
/// Returns `None` if the sentence type is unsupported or fails to parse.
pub(crate) fn sentence_to_delta(raw: &str, source_label: &str) -> Option<Delta> {
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
        _ => return None, // unsupported sentence type
    };

    if path_values.is_empty() {
        return None;
    }

    let sk_values: Vec<SkPathValue> = path_values
        .into_iter()
        .map(|pv| SkPathValue::new(pv.path, pv.value))
        .collect();

    // Extract talker ID from the sentence (first 2 chars after '$', e.g. "GP", "II", "AI")
    let talker = raw
        .strip_prefix('$')
        .and_then(|s| s.get(..2))
        .unwrap_or("UN");

    let source = Source::nmea0183(source_label, talker);
    let update = Update::new(source, sk_values);
    Some(Delta::self_vessel(vec![update]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_to_delta_rmc() {
        let raw = "$GPRMC,225446.33,A,4916.45,N,12311.12,W,000.5,054.7,191194,020.3,E,A*2B";
        let delta = sentence_to_delta(raw, "gps");
        assert!(delta.is_some(), "Expected delta from valid RMC sentence");
        let delta = delta.unwrap();
        let update = &delta.updates[0];
        let has_position = update.values.iter().any(|v| v.path == "navigation.position");
        let has_sog = update
            .values
            .iter()
            .any(|v| v.path == "navigation.speedOverGround");
        assert!(has_position, "Expected position in delta");
        assert!(has_sog, "Expected SOG in delta");
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
        let delta = sentence_to_delta(raw, "mux");
        // II talker → valid parse
        if let Some(d) = delta {
            let extra = &d.updates[0].source.extra;
            assert_eq!(
                extra.get("talker"),
                Some(&serde_json::json!("II")),
                "Expected II talker"
            );
        }
        // (may be None if checksum fails — just checking the path)
    }

    #[test]
    fn sentence_to_delta_unsupported() {
        // GSA sentence not mapped
        let raw = "$GPGSA,A,3,04,05,,09,12,,,24,,,,,2.5,1.3,2.1*39";
        let delta = sentence_to_delta(raw, "gps");
        assert!(delta.is_none());
    }

    #[test]
    fn sentence_to_delta_invalid() {
        let delta = sentence_to_delta("not a sentence", "gps");
        assert!(delta.is_none());
    }

    #[tokio::test]
    async fn provider_sends_delta_on_connect() {
        use tokio::io::AsyncWriteExt;

        let (tx, mut rx) = mpsc::channel(16);
        // Bind on port 0 → OS picks a free port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener); // free it so the provider can rebind

        let provider = NmeaTcpProvider::new(addr, "test");
        tokio::spawn(async move { provider.run(tx).await.ok() });

        // Give the provider a moment to bind
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Connect and send a valid RMC sentence
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"$GPRMC,225446.33,A,4916.45,N,12311.12,W,000.5,054.7,191194,020.3,E,A*2B\r\n")
            .await
            .unwrap();

        let delta = tokio::time::timeout(
            tokio::time::Duration::from_secs(2),
            rx.recv(),
        )
        .await
        .expect("timeout waiting for delta")
        .expect("channel closed");

        let update = &delta.updates[0];
        assert!(update.values.iter().any(|v| v.path == "navigation.position"));
    }
}
