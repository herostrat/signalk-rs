//! PGN → SignalK Delta conversion.
//!
//! Dispatches decoded NMEA 2000 messages to SignalK delta producers.
//! Navigation PGNs produce self-vessel deltas; AIS PGNs produce
//! vessel-context deltas via `AisContact::to_delta()`.

use nmea2000::DecodedMessage;
use signalk_types::{Delta, PathValue, Source, Update};

/// Source info from the raw NMEA 2000 message.
pub struct N2kSource<'a> {
    pub label: &'a str,
    pub src: u8,
    pub pgn: u32,
}

pub(crate) fn make_source(s: &N2kSource<'_>) -> Source {
    Source::nmea2000(s.label, s.src, s.pgn)
}

pub(crate) fn self_delta(values: Vec<PathValue>, source: &N2kSource<'_>) -> Option<Delta> {
    if values.is_empty() {
        return None;
    }
    Some(Delta::self_vessel(vec![Update::new(
        make_source(source),
        values,
    )]))
}

mod ais;
mod environment;
mod navigation;
mod propulsion;

use ais::*;
use environment::*;
use navigation::*;
use propulsion::*;

/// Convert a decoded NMEA 2000 message to a SignalK delta.
///
/// Returns `None` for unsupported PGNs or messages with no usable data.
pub fn decoded_to_delta(msg: &DecodedMessage, source: &N2kSource<'_>) -> Option<Delta> {
    match msg {
        // ── Navigation PGNs (self vessel) ──────────────────────────────
        DecodedMessage::VesselHeading(m) => from_vessel_heading(m, source),
        DecodedMessage::Speed(m) => from_speed(m, source),
        DecodedMessage::WaterDepth(m) => from_water_depth(m, source),
        DecodedMessage::PositionRapidUpdate(m) => from_position_rapid(m, source),
        DecodedMessage::CogSogRapidUpdate(m) => from_cog_sog_rapid(m, source),
        DecodedMessage::GnssPositionData(m) => from_gnss_position(m, source),
        DecodedMessage::RateOfTurn(m) => from_rate_of_turn(m, source),
        DecodedMessage::Attitude(m) => from_attitude(m, source),
        DecodedMessage::DistanceLog(m) => from_distance_log(m, source),
        DecodedMessage::TimeDate(m) => from_time_date(m, source),
        DecodedMessage::CrossTrackError(m) => from_cross_track_error(m, source),

        // ── Environment PGNs ───────────────────────────────────────────
        DecodedMessage::WindData(m) => from_wind_data(m, source),
        DecodedMessage::Temperature(m) => from_temperature(m, source),

        // ── Propulsion PGNs ────────────────────────────────────────────
        DecodedMessage::Rudder(m) => from_rudder(m, source),
        DecodedMessage::EngineParametersRapidUpdate(m) => from_engine_rapid(m, source),
        DecodedMessage::EngineParametersDynamic(m) => from_engine_dynamic(m, source),
        DecodedMessage::BatteryStatus(m) => from_battery_status(m, source),

        // ── AIS PGNs (vessel context by MMSI) ─────────────────────────
        DecodedMessage::AisClassAPositionReport(m) => from_ais_class_a_position(m, source),
        DecodedMessage::AisClassBPositionReport(m) => from_ais_class_b_position(m, source),
        DecodedMessage::AisClassBExtendedPositionReport(m) => from_ais_class_b_extended(m, source),
        DecodedMessage::AisClassAStaticAndVoyageRelatedData(m) => {
            from_ais_class_a_static(m, source)
        }
        DecodedMessage::AisClassBStaticDataMsg24PartA(m) => from_ais_static_24a(m, source),
        DecodedMessage::AisClassBStaticDataMsg24PartB(m) => from_ais_static_24b(m, source),

        _ => None,
    }
}
