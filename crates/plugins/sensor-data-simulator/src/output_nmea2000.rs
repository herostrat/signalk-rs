/// NMEA 2000 output — encodes SimulatedValues as PGN messages and sends via SocketCAN/vcan.
///
/// The simulator writes CAN frames to a virtual CAN interface (vcan0) or real
/// SocketCAN interface. The nmea2000-receive plugin reads from the same interface.
/// No address claiming — the simulator uses a fixed source address.
use crate::generators::SimulatedValues;
use nmea2000::PgnMessage;
use nmea2000_pgns::{
    cog_sog_rapid_update::CogSogRapidUpdate, position_rapid_update::PositionRapidUpdate,
    speed::Speed, vessel_heading::VesselHeading, water_depth::WaterDepth, wind_data::WindData,
};
use nmea2000_types::{Pgn, RawMessage};
use tracing::debug;

// ─── Encoding ───────────────────────────────────────────────────────────────

/// Encoded PGN ready to send on the CAN bus.
pub struct EncodedPgn {
    pub pgn: u32,
    pub priority: u8,
    pub data: Vec<u8>,
}

/// Encode SimulatedValues as NMEA 2000 PGN messages.
pub fn encode(values: &SimulatedValues, sid: &mut u8, enable_environment: bool) -> Vec<EncodedPgn> {
    let mut pgns = Vec::with_capacity(7);

    // PGN 129025 — Position, Rapid Update
    pgns.push(encode_pgn(
        PositionRapidUpdate::builder()
            .latitude(values.latitude)
            .longitude(values.longitude)
            .build(),
        PositionRapidUpdate::PGN.as_u32(),
        2,
    ));

    // PGN 129026 — COG & SOG, Rapid Update
    {
        let msg = CogSogRapidUpdate::builder()
            .sid(*sid as u64)
            .cog_reference_raw(0) // True
            .cog(values.cog_rad)
            .sog(values.sog_mps)
            .build();
        pgns.push(encode_pgn(msg, CogSogRapidUpdate::PGN.as_u32(), 2));
        *sid = sid.wrapping_add(1);
    }

    // PGN 127250 — Vessel Heading
    {
        let mut builder = VesselHeading::builder()
            .sid(*sid as u64)
            .heading(values.heading_magnetic_rad)
            .reference_raw(1); // Magnetic
        builder = builder.variation(values.magnetic_variation_rad);
        let msg = builder.build();
        pgns.push(encode_pgn(msg, VesselHeading::PGN.as_u32(), 2));
        *sid = sid.wrapping_add(1);
    }

    // PGN 128259 — Speed, Water Referenced
    {
        let msg = Speed::builder()
            .sid(*sid as u64)
            .speed_water_referenced(values.stw_mps)
            .speed_ground_referenced(values.sog_mps)
            .build();
        pgns.push(encode_pgn(msg, Speed::PGN.as_u32(), 2));
        *sid = sid.wrapping_add(1);
    }

    if enable_environment {
        // PGN 130306 — Wind Data (apparent)
        {
            let msg = WindData::builder()
                .sid(*sid as u64)
                .reference_raw(2) // Apparent
                .wind_speed(values.wind_speed_apparent_mps)
                .wind_angle(values.wind_angle_apparent_rad)
                .build();
            pgns.push(encode_pgn(msg, WindData::PGN.as_u32(), 2));
            *sid = sid.wrapping_add(1);
        }

        // PGN 128267 — Water Depth
        {
            let msg = WaterDepth::builder()
                .sid(*sid as u64)
                .depth(values.depth_below_transducer_m)
                .offset(values.surface_to_transducer_m)
                .build();
            pgns.push(encode_pgn(msg, WaterDepth::PGN.as_u32(), 3));
            *sid = sid.wrapping_add(1);
        }
    }

    // Filter out any that failed to encode
    pgns.into_iter().flatten().collect()
}

fn encode_pgn<M: PgnMessage>(msg: M, pgn: u32, priority: u8) -> Option<EncodedPgn> {
    let mut buf = vec![0u8; msg.data_length()];
    match msg.encode(&mut buf) {
        Ok(len) => {
            buf.truncate(len);
            Some(EncodedPgn {
                pgn,
                priority,
                data: buf,
            })
        }
        Err(e) => {
            debug!(pgn, "PGN encode error: {e:?}");
            None
        }
    }
}

// ─── Transport ──────────────────────────────────────────────────────────────

/// Sends encoded PGN messages via SocketCAN.
///
/// Runs in a blocking thread (SocketCAN is synchronous).
/// Receives encoded PGNs via mpsc channel from the async tick task.
pub fn run_bus_writer(
    interface: &str,
    source: u8,
    rx: std::sync::mpsc::Receiver<Vec<EncodedPgn>>,
) {
    use nmea2000::N2kTransport;

    loop {
        let mut bus = match nmea2000::N2kBus::open(interface) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(interface, error = %e, "vcan open failed, retrying in 5s");
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
        };

        tracing::info!(interface, source, "NMEA 2000 simulator output connected");

        loop {
            match rx.recv() {
                Ok(pgns) => {
                    for encoded in pgns {
                        let msg = RawMessage {
                            pgn: Pgn::new(encoded.pgn),
                            source,
                            destination: None,
                            priority: encoded.priority,
                            data: encoded.data,
                        };
                        if let Err(e) = bus.send_message(&msg) {
                            debug!(pgn = encoded.pgn, error = %e, "CAN send failed");
                            break;
                        }
                    }
                }
                Err(_) => return, // channel closed
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators;
    use nmea2000::DecodedMessage;

    fn test_values() -> SimulatedValues {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, true);
        state.tick_at(42.0)
    }

    #[test]
    fn encode_produces_all_pgns_with_environment() {
        let values = test_values();
        let mut sid = 0u8;
        let pgns = encode(&values, &mut sid, true);

        // Position, COG/SOG, Heading, Speed, Wind, Depth = 6
        assert_eq!(pgns.len(), 6, "expected 6 PGNs, got {}", pgns.len());

        let pgn_ids: Vec<u32> = pgns.iter().map(|p| p.pgn).collect();
        assert!(pgn_ids.contains(&129025), "missing Position Rapid Update");
        assert!(pgn_ids.contains(&129026), "missing COG/SOG");
        assert!(pgn_ids.contains(&127250), "missing Vessel Heading");
        assert!(pgn_ids.contains(&128259), "missing Speed");
        assert!(pgn_ids.contains(&130306), "missing Wind Data");
        assert!(pgn_ids.contains(&128267), "missing Water Depth");
    }

    #[test]
    fn encode_without_environment() {
        let values = test_values();
        let mut sid = 0u8;
        let pgns = encode(&values, &mut sid, false);

        // Position, COG/SOG, Heading, Speed = 4
        assert_eq!(pgns.len(), 4, "expected 4 PGNs, got {}", pgns.len());

        let pgn_ids: Vec<u32> = pgns.iter().map(|p| p.pgn).collect();
        assert!(!pgn_ids.contains(&130306), "should not have Wind Data");
        assert!(!pgn_ids.contains(&128267), "should not have Water Depth");
    }

    #[test]
    fn position_roundtrip() {
        let values = test_values();
        let mut sid = 0u8;
        let pgns = encode(&values, &mut sid, false);
        let pos = pgns.iter().find(|p| p.pgn == 129025).unwrap();

        let decoded = PositionRapidUpdate::decode(&pos.data).unwrap();
        let lat = decoded.latitude().unwrap();
        let lon = decoded.longitude().unwrap();
        assert!(
            (lat - values.latitude).abs() < 0.0001,
            "lat: {lat} vs {}",
            values.latitude
        );
        assert!(
            (lon - values.longitude).abs() < 0.0001,
            "lon: {lon} vs {}",
            values.longitude
        );
    }

    #[test]
    fn cog_sog_roundtrip() {
        let values = test_values();
        let mut sid = 0u8;
        let pgns = encode(&values, &mut sid, false);
        let cs = pgns.iter().find(|p| p.pgn == 129026).unwrap();

        let decoded = CogSogRapidUpdate::decode(&cs.data).unwrap();
        let cog = decoded.cog().unwrap();
        let sog = decoded.sog().unwrap();
        assert!((cog - values.cog_rad).abs() < 0.001, "cog: {cog}");
        assert!((sog - values.sog_mps).abs() < 0.01, "sog: {sog}");
    }

    #[test]
    fn heading_roundtrip() {
        let values = test_values();
        let mut sid = 0u8;
        let pgns = encode(&values, &mut sid, false);
        let hdg = pgns.iter().find(|p| p.pgn == 127250).unwrap();

        let decoded = VesselHeading::decode(&hdg.data).unwrap();
        let heading = decoded.heading().unwrap();
        assert!(
            (heading - values.heading_magnetic_rad).abs() < 0.001,
            "heading: {heading}"
        );
    }

    #[test]
    fn speed_roundtrip() {
        let values = test_values();
        let mut sid = 0u8;
        let pgns = encode(&values, &mut sid, false);
        let spd = pgns.iter().find(|p| p.pgn == 128259).unwrap();

        let decoded = Speed::decode(&spd.data).unwrap();
        let stw = decoded.speed_water_referenced().unwrap();
        assert!((stw - values.stw_mps).abs() < 0.01, "stw: {stw}");
    }

    #[test]
    fn wind_roundtrip() {
        let values = test_values();
        let mut sid = 0u8;
        let pgns = encode(&values, &mut sid, true);
        let wind = pgns.iter().find(|p| p.pgn == 130306).unwrap();

        let decoded = WindData::decode(&wind.data).unwrap();
        let speed = decoded.wind_speed().unwrap();
        let angle = decoded.wind_angle().unwrap();
        assert!(
            (speed - values.wind_speed_apparent_mps).abs() < 0.01,
            "wind speed: {speed}"
        );
        assert!(
            (angle - values.wind_angle_apparent_rad).abs() < 0.001,
            "wind angle: {angle}"
        );
    }

    #[test]
    fn depth_roundtrip() {
        let values = test_values();
        let mut sid = 0u8;
        let pgns = encode(&values, &mut sid, true);
        let depth = pgns.iter().find(|p| p.pgn == 128267).unwrap();

        let decoded = WaterDepth::decode(&depth.data).unwrap();
        let d = decoded.depth().unwrap();
        assert!(
            (d - values.depth_below_transducer_m).abs() < 0.01,
            "depth: {d}"
        );
    }

    #[test]
    fn sid_increments() {
        let values = test_values();
        let mut sid = 0u8;
        encode(&values, &mut sid, true);
        // COG/SOG, Heading, Speed, Wind, Depth each increment SID = 5
        assert_eq!(sid, 5, "SID should be 5 after encoding");
    }

    #[test]
    fn sid_wraps() {
        let values = test_values();
        let mut sid = 253u8;
        encode(&values, &mut sid, true);
        // 253 → 254 → 255 → 0 → 1 → 2
        assert_eq!(sid, 2, "SID should wrap around");
    }

    #[test]
    fn all_pgns_decode_via_decoded_message() {
        let values = test_values();
        let mut sid = 0u8;
        let pgns = encode(&values, &mut sid, true);

        for encoded in &pgns {
            let result = DecodedMessage::decode(encoded.pgn, &encoded.data);
            assert!(
                result.is_ok(),
                "PGN {} failed to decode: {:?}",
                encoded.pgn,
                result.err()
            );
        }
    }
}
