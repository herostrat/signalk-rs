/// NMEA 0183 output — encodes SimulatedValues as NMEA sentences and sends via TCP.
///
/// The simulator acts as a TCP **client** connecting to the nmea0183-receive
/// plugin's TCP listener. Sentences are plain ASCII with XOR checksum.
use crate::generators::SimulatedValues;
use std::f64::consts::PI;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tracing::{debug, warn};

const MPS_TO_KNOTS: f64 = 1.943_844;
const FEET_PER_METER: f64 = 3.280_84;
const FATHOMS_PER_METER: f64 = 0.546_807;

// ─── Encoding ───────────────────────────────────────────────────────────────

/// Compute NMEA checksum (XOR of all bytes between '$' and '*', exclusive).
fn checksum(body: &str) -> u8 {
    body.bytes().fold(0u8, |acc, b| acc ^ b)
}

/// Format a complete NMEA sentence: `$body*CC\r\n`
fn sentence(body: &str) -> String {
    format!("${}*{:02X}\r\n", body, checksum(body))
}

/// Convert decimal degrees to NMEA DDMM.MMMM / DDDMM.MMMM format.
fn deg_to_nmea_lat(deg: f64) -> (String, char) {
    let hemisphere = if deg >= 0.0 { 'N' } else { 'S' };
    let deg = deg.abs();
    let d = deg.floor() as u32;
    let m = (deg - d as f64) * 60.0;
    (format!("{:02}{:07.4}", d, m), hemisphere)
}

fn deg_to_nmea_lon(deg: f64) -> (String, char) {
    let hemisphere = if deg >= 0.0 { 'E' } else { 'W' };
    let deg = deg.abs();
    let d = deg.floor() as u32;
    let m = (deg - d as f64) * 60.0;
    (format!("{:03}{:07.4}", d, m), hemisphere)
}

/// Encode all SimulatedValues as NMEA 0183 sentences.
pub fn encode(values: &SimulatedValues, talker: &str, enable_environment: bool) -> Vec<String> {
    let mut sentences = Vec::with_capacity(10);
    let now = chrono::Utc::now().naive_utc();

    // RMC — Recommended Minimum Specific GNSS Data
    {
        let (lat_str, lat_hem) = deg_to_nmea_lat(values.latitude);
        let (lon_str, lon_hem) = deg_to_nmea_lon(values.longitude);
        let sog_kn = values.sog_mps * MPS_TO_KNOTS;
        let cog_deg = values.cog_rad * 180.0 / PI;
        let var_deg = values.magnetic_variation_rad.abs() * 180.0 / PI;
        let var_hem = if values.magnetic_variation_rad >= 0.0 {
            'E'
        } else {
            'W'
        };
        let time = now.time();
        let date = now.date();
        sentences.push(sentence(&format!(
            "{talker}RMC,{:02}{:02}{:05.2},A,{lat_str},{lat_hem},{lon_str},{lon_hem},{sog_kn:.1},{cog_deg:.1},{:02}{:02}{:02},{var_deg:.1},{var_hem},A",
            time.format("%H").to_string().parse::<u32>().unwrap_or(0),
            time.format("%M").to_string().parse::<u32>().unwrap_or(0),
            now.and_utc().timestamp() as f64 % 60.0 + now.and_utc().timestamp_subsec_millis() as f64 / 1000.0,
            date.format("%d"),
            date.format("%m"),
            date.format("%y"),
        )));
    }

    // GGA — Global Positioning System Fix Data
    {
        let (lat_str, lat_hem) = deg_to_nmea_lat(values.latitude);
        let (lon_str, lon_hem) = deg_to_nmea_lon(values.longitude);
        let time = now.time();
        sentences.push(sentence(&format!(
            "{talker}GGA,{:02}{:02}{:05.2},{lat_str},{lat_hem},{lon_str},{lon_hem},1,08,1.2,0.0,M,0.0,M,,",
            time.format("%H").to_string().parse::<u32>().unwrap_or(0),
            time.format("%M").to_string().parse::<u32>().unwrap_or(0),
            now.and_utc().timestamp() as f64 % 60.0 + now.and_utc().timestamp_subsec_millis() as f64 / 1000.0,
        )));
    }

    // HDG — Heading, Deviation & Variation (magnetic)
    {
        let heading_deg = values.heading_magnetic_rad * 180.0 / PI;
        let var_deg = values.magnetic_variation_rad.abs() * 180.0 / PI;
        let var_hem = if values.magnetic_variation_rad >= 0.0 {
            'E'
        } else {
            'W'
        };
        sentences.push(sentence(&format!(
            "{talker}HDG,{heading_deg:.1},,,,{var_deg:.1},{var_hem}"
        )));
    }

    // VHW — Water Speed and Heading
    {
        let heading_true_deg = (values.heading_magnetic_rad + values.magnetic_variation_rad)
            .rem_euclid(2.0 * PI)
            * 180.0
            / PI;
        let heading_mag_deg = values.heading_magnetic_rad * 180.0 / PI;
        let stw_kn = values.stw_mps * MPS_TO_KNOTS;
        let stw_kmh = values.stw_mps * 3.6;
        sentences.push(sentence(&format!(
            "{talker}VHW,{heading_true_deg:.1},T,{heading_mag_deg:.1},M,{stw_kn:.1},N,{stw_kmh:.1},K"
        )));
    }

    // MWV — Wind Speed and Angle (apparent)
    if enable_environment {
        let angle_deg = values.wind_angle_apparent_rad * 180.0 / PI;
        let speed_kn = values.wind_speed_apparent_mps * MPS_TO_KNOTS;
        sentences.push(sentence(&format!(
            "{talker}MWV,{angle_deg:.1},R,{speed_kn:.1},N,A"
        )));
    }

    // DBT — Depth Below Transducer
    if enable_environment {
        let depth_m = values.depth_below_transducer_m;
        let depth_ft = depth_m * FEET_PER_METER;
        let depth_fa = depth_m * FATHOMS_PER_METER;
        sentences.push(sentence(&format!(
            "{talker}DBT,{depth_ft:.1},f,{depth_m:.1},M,{depth_fa:.1},F"
        )));
    }

    // MTW — Mean Temperature of Water
    if enable_environment {
        let water_temp_c = values.water_temperature_k - 273.15;
        sentences.push(sentence(&format!("{talker}MTW,{water_temp_c:.1},C")));
    }

    // MDA — Meteorological Composite
    if enable_environment {
        let pressure_bar = values.pressure_pa / 100_000.0;
        let air_temp_c = values.air_temperature_k - 273.15;
        let humidity_pct = values.humidity_ratio * 100.0;
        // MDA format: $--MDA,x.x,I,x.x,B,x.x,C,x.x,C,x.x,,x.x,C,x.x,T,x.x,M,x.x,N,x.x,M*hh
        // Simplified: barometric pressure (bars), air temp, humidity
        sentences.push(sentence(&format!(
            "{talker}MDA,,I,{pressure_bar:.4},B,{air_temp_c:.1},C,,,{humidity_pct:.1},,,,,,,,,"
        )));
    }

    // XDR — Transducer Measurements (pitch + roll)
    {
        let pitch_deg = values.pitch_rad * 180.0 / PI;
        let roll_deg = values.roll_rad * 180.0 / PI;
        sentences.push(sentence(&format!(
            "{talker}XDR,A,{pitch_deg:.1},D,PITCH,A,{roll_deg:.1},D,ROLL"
        )));
    }

    sentences
}

// ─── TCP Client ─────────────────────────────────────────────────────────────

pub struct Nmea0183Output {
    stream: Option<TcpStream>,
    host: String,
    port: u16,
    talker: String,
    enable_environment: bool,
}

impl Nmea0183Output {
    pub fn new(host: String, port: u16, talker: String, enable_environment: bool) -> Self {
        Nmea0183Output {
            stream: None,
            host,
            port,
            talker,
            enable_environment,
        }
    }

    /// Encode values and send all sentences to the TCP server.
    pub async fn send(&mut self, values: &SimulatedValues) {
        if self.stream.is_none()
            && let Err(e) = self.connect().await
        {
            debug!(error = %e, "NMEA 0183 TCP connect failed");
            return;
        }

        let sentences = encode(values, &self.talker, self.enable_environment);
        let stream = self.stream.as_mut().unwrap();

        for s in &sentences {
            if let Err(e) = stream.write_all(s.as_bytes()).await {
                warn!(error = %e, "NMEA 0183 TCP write failed, will reconnect");
                self.stream = None;
                return;
            }
        }
    }

    async fn connect(&mut self) -> Result<(), std::io::Error> {
        let addr = format!("{}:{}", self.host, self.port);
        let stream = TcpStream::connect(&addr).await?;
        debug!(addr = %addr, "NMEA 0183 TCP connected");
        self.stream = Some(stream);
        Ok(())
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators;

    fn test_values() -> SimulatedValues {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, true);
        state.tick_at(42.0)
    }

    #[test]
    fn checksum_computation() {
        // Verify XOR checksum: 'G'^'P'^'G'^'L'^'L' = 0x47^0x50^0x47^0x4C^0x4C = ...
        // Use a simple known case
        let body = "GPGLL,5300.97914,N,00259.98174,E";
        let cs = checksum(body);
        // Verify it round-trips: our checksum function must match what we embed
        assert_eq!(cs, body.bytes().fold(0u8, |acc, b| acc ^ b));
        // Verify the sentence format works
        let s = sentence(body);
        assert!(s.starts_with('$'));
        assert!(s.contains(&format!("*{cs:02X}")));
    }

    #[test]
    fn sentence_format() {
        let s = sentence("GPGLL,5300.97914,N,00259.98174,E");
        assert!(s.starts_with('$'));
        assert!(s.ends_with("\r\n"));
        assert!(s.contains('*'));
    }

    #[test]
    fn lat_lon_conversion() {
        // 54.5° N → 5430.0000,N
        let (lat, hem) = deg_to_nmea_lat(54.5);
        assert_eq!(hem, 'N');
        assert_eq!(lat, "5430.0000");

        // 10.0° E → 01000.0000,E
        let (lon, hem) = deg_to_nmea_lon(10.0);
        assert_eq!(hem, 'E');
        assert_eq!(lon, "01000.0000");

        // Negative: -33.8688° S
        let (lat, hem) = deg_to_nmea_lat(-33.8688);
        assert_eq!(hem, 'S');
        assert!(lat.starts_with("33"));
    }

    #[test]
    fn encode_produces_all_sentences() {
        let values = test_values();
        let sentences = encode(&values, "GP", true);

        // Should have: RMC, GGA, HDG, VHW, MWV, DBT, MTW, MDA, XDR = 9
        assert_eq!(
            sentences.len(),
            9,
            "expected 9 sentences, got {}",
            sentences.len()
        );

        // Check sentence types present
        let types: Vec<&str> = sentences
            .iter()
            .map(|s| &s[3..6]) // $GPXXX → XXX
            .collect();
        assert!(types.contains(&"RMC"));
        assert!(types.contains(&"GGA"));
        assert!(types.contains(&"HDG"));
        assert!(types.contains(&"VHW"));
        assert!(types.contains(&"MWV"));
        assert!(types.contains(&"DBT"));
        assert!(types.contains(&"MTW"));
        assert!(types.contains(&"MDA"));
        assert!(types.contains(&"XDR"));
    }

    #[test]
    fn encode_without_environment() {
        let values = test_values();
        let sentences = encode(&values, "GP", false);

        // Without environment: RMC, GGA, HDG, VHW, XDR = 5
        assert_eq!(
            sentences.len(),
            5,
            "expected 5 sentences, got {}",
            sentences.len()
        );

        let types: Vec<&str> = sentences.iter().map(|s| &s[3..6]).collect();
        assert!(!types.contains(&"MWV"));
        assert!(!types.contains(&"DBT"));
        assert!(!types.contains(&"MTW"));
        assert!(!types.contains(&"MDA"));
    }

    #[test]
    fn rmc_has_valid_fix() {
        let values = test_values();
        let sentences = encode(&values, "GP", false);
        let rmc = sentences.iter().find(|s| s[3..6] == *"RMC").unwrap();
        // Should contain 'A' for valid fix
        let fields: Vec<&str> = rmc[1..rmc.find('*').unwrap()].split(',').collect();
        assert_eq!(fields[2], "A", "RMC status should be Active");
    }

    #[test]
    fn all_sentences_have_valid_checksum() {
        let values = test_values();
        let sentences = encode(&values, "GP", true);

        for s in &sentences {
            assert!(s.starts_with('$'), "should start with $: {s}");
            assert!(s.ends_with("\r\n"), "should end with CRLF: {s}");

            let star_pos = s.find('*').expect("should have *");
            let body = &s[1..star_pos];
            let expected_cs = checksum(body);
            let actual_cs = &s[star_pos + 1..star_pos + 3];
            assert_eq!(
                actual_cs,
                format!("{expected_cs:02X}"),
                "checksum mismatch for {s}"
            );
        }
    }

    #[test]
    fn custom_talker_id() {
        let values = test_values();
        let sentences = encode(&values, "II", true);

        for s in &sentences {
            assert!(s.starts_with("$II"), "should use talker II: {s}");
        }
    }

    #[test]
    fn dbt_depth_units() {
        let values = test_values();
        let sentences = encode(&values, "GP", true);
        let dbt = sentences.iter().find(|s| s[3..6] == *"DBT").unwrap();
        let fields: Vec<&str> = dbt[1..dbt.find('*').unwrap()].split(',').collect();
        // Format: GPDBT,feet,f,meters,M,fathoms,F
        assert_eq!(fields[2], "f");
        assert_eq!(fields[4], "M");
        assert_eq!(fields[6], "F");
        // Verify unit conversion: feet ≈ meters * 3.28084
        let feet: f64 = fields[1].parse().unwrap();
        let meters: f64 = fields[3].parse().unwrap();
        assert!((feet - meters * FEET_PER_METER).abs() < 0.2);
    }

    #[test]
    fn xdr_pitch_roll() {
        let values = test_values();
        let sentences = encode(&values, "GP", false);
        let xdr = sentences.iter().find(|s| s[3..6] == *"XDR").unwrap();
        let fields: Vec<&str> = xdr[1..xdr.find('*').unwrap()].split(',').collect();
        // Format: GPXDR,A,pitch,D,PITCH,A,roll,D,ROLL
        assert_eq!(fields[1], "A"); // transducer type: angular
        assert_eq!(fields[3], "D"); // units: degrees
        assert_eq!(fields[4], "PITCH");
        assert_eq!(fields[5], "A");
        assert_eq!(fields[7], "D");
        assert_eq!(fields[8], "ROLL");
    }

    #[test]
    fn hdg_heading_magnetic() {
        let values = test_values();
        let sentences = encode(&values, "GP", false);
        let hdg = sentences.iter().find(|s| s[3..6] == *"HDG").unwrap();
        let fields: Vec<&str> = hdg[1..hdg.find('*').unwrap()].split(',').collect();
        // Format: GPHDG,heading,,,,variation,E/W
        let heading: f64 = fields[1].parse().unwrap();
        assert!((0.0..360.0).contains(&heading), "heading = {heading}");
    }

    #[test]
    fn rmc_position_matches_input() {
        let values = test_values();
        let sentences = encode(&values, "GP", false);
        let rmc = sentences.iter().find(|s| s[3..6] == *"RMC").unwrap();
        let fields: Vec<&str> = rmc[1..rmc.find('*').unwrap()].split(',').collect();

        // Parse lat: DDMM.MMMM
        let lat_str = fields[3];
        let lat_deg: f64 =
            lat_str[..2].parse::<f64>().unwrap() + lat_str[2..].parse::<f64>().unwrap() / 60.0;
        let lat_sign = if fields[4] == "N" { 1.0 } else { -1.0 };
        let lat = lat_deg * lat_sign;

        // Parse lon: DDDMM.MMMM
        let lon_str = fields[5];
        let lon_deg: f64 =
            lon_str[..3].parse::<f64>().unwrap() + lon_str[3..].parse::<f64>().unwrap() / 60.0;
        let lon_sign = if fields[6] == "E" { 1.0 } else { -1.0 };
        let lon = lon_deg * lon_sign;

        assert!(
            (lat - values.latitude).abs() < 0.001,
            "RMC lat {lat} != input {}",
            values.latitude
        );
        assert!(
            (lon - values.longitude).abs() < 0.001,
            "RMC lon {lon} != input {}",
            values.longitude
        );
    }

    #[test]
    fn rmc_sog_matches_input() {
        let values = test_values();
        let sentences = encode(&values, "GP", false);
        let rmc = sentences.iter().find(|s| s[3..6] == *"RMC").unwrap();
        let fields: Vec<&str> = rmc[1..rmc.find('*').unwrap()].split(',').collect();

        let sog_kn: f64 = fields[7].parse().unwrap();
        let sog_mps = sog_kn / MPS_TO_KNOTS;
        assert!(
            (sog_mps - values.sog_mps).abs() < 0.1,
            "SOG: NMEA {sog_mps} m/s != input {} m/s",
            values.sog_mps
        );
    }

    #[test]
    fn all_three_outputs_cover_same_navigation_paths() {
        // Verify that direct, NMEA 0183, and NMEA 2000 outputs
        // all produce data for the key navigation paths
        use crate::output_direct;
        use crate::output_nmea2000;

        let values = test_values();

        // Direct: check paths present
        let delta = output_direct::build_delta(&values, true);
        let direct_paths: Vec<&str> = delta.updates[0]
            .values
            .iter()
            .map(|pv| pv.path.as_str())
            .collect();

        // NMEA 0183: check sentence types present
        let nmea_sentences = encode(&values, "GP", true);
        let nmea_types: Vec<&str> = nmea_sentences.iter().map(|s| &s[3..6]).collect();

        // NMEA 2000: check PGN types present
        let mut sid = 0u8;
        let pgns = output_nmea2000::encode(&values, &mut sid, true);
        let pgn_ids: Vec<u32> = pgns.iter().map(|p| p.pgn).collect();

        // Position: all three must provide it
        assert!(direct_paths.contains(&"navigation.position"));
        assert!(nmea_types.contains(&"RMC") || nmea_types.contains(&"GGA"));
        assert!(pgn_ids.contains(&129025)); // Position Rapid Update

        // SOG/COG: all three
        assert!(direct_paths.contains(&"navigation.speedOverGround"));
        assert!(nmea_types.contains(&"RMC")); // RMC has SOG
        assert!(pgn_ids.contains(&129026)); // COG/SOG

        // Heading: all three
        assert!(direct_paths.contains(&"navigation.headingMagnetic"));
        assert!(nmea_types.contains(&"HDG"));
        assert!(pgn_ids.contains(&127250)); // Vessel Heading

        // Depth: all three (when environment enabled)
        assert!(direct_paths.contains(&"environment.depth.belowTransducer"));
        assert!(nmea_types.contains(&"DBT"));
        assert!(pgn_ids.contains(&128267)); // Water Depth

        // Wind: all three (when environment enabled)
        assert!(direct_paths.contains(&"environment.wind.speedApparent"));
        assert!(nmea_types.contains(&"MWV"));
        assert!(pgn_ids.contains(&130306)); // Wind Data
    }
}
