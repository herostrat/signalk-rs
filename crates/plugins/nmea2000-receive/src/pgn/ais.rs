//! AIS PGNs: Class A/B position reports, static data (PGNs 129038–129810)
use super::{N2kSource, make_source};
use nmea2000::pgns::*;
use signalk_types::Delta;
use signalk_types::ais::AisContact;

// ─── PGN 129038: AIS Class A Position Report ─────────────────────────────────

pub(super) fn from_ais_class_a_position(
    m: &ais_class_a_position_report::AisClassAPositionReport,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        position: match (m.latitude(), m.longitude()) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        },
        sog_ms: m.sog(),
        cog_rad: m.cog(),
        heading_rad: m.heading(),
        rot_rads: m.rate_of_turn(),
        nav_status: m.nav_status_raw().map(|v| v as u8),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

// ─── PGN 129039: AIS Class B Position Report ─────────────────────────────────

pub(super) fn from_ais_class_b_position(
    m: &ais_class_b_position_report::AisClassBPositionReport,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        position: match (m.latitude(), m.longitude()) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        },
        sog_ms: m.sog(),
        cog_rad: m.cog(),
        heading_rad: m.heading(),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

// ─── PGN 129040: AIS Class B Extended Position Report ────────────────────────

pub(super) fn from_ais_class_b_extended(
    m: &ais_class_b_extended_position_report::AisClassBExtendedPositionReport,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        position: match (m.latitude(), m.longitude()) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        },
        sog_ms: m.sog(),
        cog_rad: m.cog(),
        heading_rad: m.true_heading(),
        name: m.name().map(str::to_string),
        ship_type: m.type_of_ship_raw().map(|v| v as u8),
        length: m.length().filter(|&v| v > 0.0),
        beam: m.beam().filter(|&v| v > 0.0),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

// ─── PGN 129794: AIS Class A Static and Voyage Related Data ──────────────────

pub(super) fn from_ais_class_a_static(
    m: &ais_class_a_static_and_voyage_related_data::AisClassAStaticAndVoyageRelatedData,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        name: m.name().map(str::to_string),
        callsign: m.callsign().map(str::to_string),
        imo: m.imo_number().map(|v| v as u32),
        ship_type: m.type_of_ship_raw().map(|v| v as u8),
        destination: m.destination().map(str::to_string),
        draught: m.draft(),
        length: m.length().filter(|&v| v > 0.0),
        beam: m.beam().filter(|&v| v > 0.0),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

// ─── PGN 129809: AIS Class B Static Data, Part A ─────────────────────────────

pub(super) fn from_ais_static_24a(
    m: &ais_class_b_static_data_msg24_part_a::AisClassBStaticDataMsg24PartA,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        name: m.name().map(str::to_string),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

// ─── PGN 129810: AIS Class B Static Data, Part B ─────────────────────────────

pub(super) fn from_ais_static_24b(
    m: &ais_class_b_static_data_msg24_part_b::AisClassBStaticDataMsg24PartB,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mmsi = m.user_id()? as u32;
    let contact = AisContact {
        mmsi,
        callsign: m.callsign().map(str::to_string),
        ship_type: m.type_of_ship_raw().map(|v| v as u8),
        length: m.length().filter(|&v| v > 0.0),
        beam: m.beam().filter(|&v| v > 0.0),
        ..Default::default()
    };
    Some(contact.to_delta(make_source(source)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nmea2000::DecodedMessage;

    fn test_source(pgn: u32) -> N2kSource<'static> {
        N2kSource {
            label: "n2k",
            src: 0,
            pgn,
        }
    }

    #[test]
    fn ais_class_a_position_report() {
        let msg = ais_class_a_position_report::AisClassAPositionReport::builder()
            .user_id_raw(211457160)
            .latitude(54.3)
            .longitude(10.5)
            .sog(5.0)
            .cog(1.0)
            .heading(1.1)
            .nav_status_raw(0)
            .build();
        let decoded = DecodedMessage::AisClassAPositionReport(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(129038)).unwrap();
        assert!(
            delta
                .context
                .as_deref()
                .unwrap()
                .starts_with("vessels.urn:mrn:imo:mmsi:211457160")
        );
        let values = &delta.updates[0].values;
        assert!(values.iter().any(|v| v.path == "navigation.position"));
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.speedOverGround")
        );
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.courseOverGroundTrue")
        );
        assert!(values.iter().any(|v| v.path == "navigation.state"));
    }

    #[test]
    fn ais_class_b_position_report() {
        let msg = ais_class_b_position_report::AisClassBPositionReport::builder()
            .user_id_raw(366999000)
            .latitude(37.8)
            .longitude(-122.4)
            .sog(2.5)
            .build();
        let decoded = DecodedMessage::AisClassBPositionReport(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(129039)).unwrap();
        assert!(delta.context.as_deref().unwrap().contains("366999000"));
        let values = &delta.updates[0].values;
        assert!(values.iter().any(|v| v.path == "navigation.position"));
        assert!(
            values
                .iter()
                .any(|v| v.path == "navigation.speedOverGround")
        );
    }
}
