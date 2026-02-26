/// NMEA 0183 serial-port input provider.
///
/// Opens a serial device (e.g. `/dev/ttyUSB0`) and reads NMEA 0183 sentences
/// line by line. On read errors the port is closed and the provider retries
/// after a 5 s back-off, so it gracefully survives device unplug/replug cycles.
///
/// Spawning strategy:
/// - One `tokio::spawn` is used by the caller (main.rs) for the whole provider.
/// - Unlike the TCP provider there is no per-connection spawning; serial is a
///   single stream.
use crate::provider::sentence_to_delta;
use anyhow::Result;
use signalk_types::Delta;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Provider that reads NMEA 0183 sentences from a serial port and emits Deltas.
pub struct NmeaSerialProvider {
    /// Serial device path, e.g. `/dev/ttyUSB0` or `/dev/ttyS0`.
    pub path: String,
    /// Baud rate — standard NMEA 0183 is 4800; high-speed muxes often use 38400.
    pub baud_rate: u32,
    /// Source label reported in SignalK deltas (e.g. `"gps"`, `"depth-sensor"`).
    pub source_label: String,
}

impl NmeaSerialProvider {
    pub fn new(
        path: impl Into<String>,
        baud_rate: u32,
        source_label: impl Into<String>,
    ) -> Self {
        NmeaSerialProvider {
            path: path.into(),
            baud_rate,
            source_label: source_label.into(),
        }
    }

    /// Open the serial port and start reading sentences.
    ///
    /// On a read error the port is closed and re-opened after a 5 s delay.
    /// Returns when `tx` is closed (all receivers dropped).
    pub async fn run(self, tx: mpsc::Sender<Delta>) -> Result<()> {
        info!(path = %self.path, baud_rate = self.baud_rate, "NMEA serial provider starting");

        loop {
            if tx.is_closed() {
                break;
            }

            match self.open_and_read(&tx).await {
                Ok(()) => {
                    // Clean exit (tx closed)
                    break;
                }
                Err(e) => {
                    error!(path = %self.path, "Serial error: {e} — retrying in 5 s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            }
        }

        Ok(())
    }

    async fn open_and_read(&self, tx: &mpsc::Sender<Delta>) -> Result<()> {
        use tokio_serial::SerialPortBuilderExt;

        let port = tokio_serial::new(&self.path, self.baud_rate).open_native_async()?;
        info!(path = %self.path, "Serial port opened");

        let mut lines = BufReader::new(port).lines();

        while let Some(line) = lines.next_line().await? {
            let sentence = line.trim().to_string();
            if sentence.is_empty() {
                continue;
            }
            debug!(sentence = %sentence, "NMEA serial sentence");

            if let Some(delta) = sentence_to_delta(&sentence, &self.source_label)
                && tx.send(delta).await.is_err()
            {
                break; // receiver dropped → shut down
            }
        }

        warn!(path = %self.path, "Serial port EOF / closed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_new_stores_fields() {
        let p = NmeaSerialProvider::new("/dev/ttyUSB0", 4800, "gps");
        assert_eq!(p.path, "/dev/ttyUSB0");
        assert_eq!(p.baud_rate, 4800);
        assert_eq!(p.source_label, "gps");
    }

    #[test]
    fn provider_high_speed() {
        let p = NmeaSerialProvider::new("/dev/ttyS0", 38400, "ais");
        assert_eq!(p.baud_rate, 38400);
    }
}
