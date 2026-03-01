//! Propulsion PGNs: Rudder (127245), EngineParametersRapidUpdate (127488),
//! EngineParametersDynamic (127489), BatteryStatus (127508)
use super::{N2kSource, self_delta};
use nmea2000::pgns::*;
use signalk_types::{Delta, PathValue};

/// Map engine instance raw value to a path prefix.
/// SingleEngineOrDualEnginePort (0) → "propulsion.0"
/// DualEngineStarboard (1) → "propulsion.1"
fn engine_prefix(instance_raw: Option<u64>) -> String {
    match instance_raw {
        Some(n) => format!("propulsion.{n}"),
        None => "propulsion.0".to_string(),
    }
}

// ─── PGN 127245: Rudder ───────────────────────────────────────────────────────

pub(super) fn from_rudder(m: &rudder::Rudder, source: &N2kSource<'_>) -> Option<Delta> {
    let pos = m.position()?;
    self_delta(
        vec![PathValue::new(
            "steering.rudderAngle",
            serde_json::json!(pos),
        )],
        source,
    )
}

// ─── PGN 127488: Engine Parameters, Rapid Update ─────────────────────────────

pub(super) fn from_engine_rapid(
    m: &engine_parameters_rapid_update::EngineParametersRapidUpdate,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mut values = Vec::new();
    let prefix = engine_prefix(m.instance_raw());

    if let Some(rpm) = m.speed() {
        // RPM → rad/s: rpm × 2π / 60
        values.push(PathValue::new(
            format!("{prefix}.revolutions"),
            serde_json::json!(rpm * std::f64::consts::TAU / 60.0),
        ));
    }

    self_delta(values, source)
}

// ─── PGN 127489: Engine Parameters, Dynamic ───────────────────────────────────

pub(super) fn from_engine_dynamic(
    m: &engine_parameters_dynamic::EngineParametersDynamic,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mut values = Vec::new();
    let prefix = engine_prefix(m.instance_raw());

    if let Some(p) = m.oil_pressure() {
        values.push(PathValue::new(
            format!("{prefix}.oilPressure"),
            serde_json::json!(p),
        ));
    }
    if let Some(t) = m.temperature() {
        // .temperature() in PGN 127489 is coolant temperature
        values.push(PathValue::new(
            format!("{prefix}.coolantTemperature"),
            serde_json::json!(t),
        ));
    }
    if let Some(t) = m.oil_temperature() {
        values.push(PathValue::new(
            format!("{prefix}.oilTemperature"),
            serde_json::json!(t),
        ));
    }
    if let Some(v) = m.alternator_potential() {
        values.push(PathValue::new(
            format!("{prefix}.alternatorVoltage"),
            serde_json::json!(v),
        ));
    }

    self_delta(values, source)
}

// ─── PGN 127508: Battery Status ───────────────────────────────────────────────

pub(super) fn from_battery_status(
    m: &battery_status::BatteryStatus,
    source: &N2kSource<'_>,
) -> Option<Delta> {
    let mut values = Vec::new();
    let instance = m.instance().unwrap_or(0);
    let prefix = format!("electrical.batteries.{instance}");

    if let Some(v) = m.voltage() {
        values.push(PathValue::new(
            format!("{prefix}.voltage"),
            serde_json::json!(v),
        ));
    }
    if let Some(a) = m.current() {
        values.push(PathValue::new(
            format!("{prefix}.current"),
            serde_json::json!(a),
        ));
    }
    if let Some(t) = m.temperature() {
        values.push(PathValue::new(
            format!("{prefix}.temperature"),
            serde_json::json!(t),
        ));
    }

    self_delta(values, source)
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
    fn rudder_angle() {
        let msg = rudder::Rudder::builder()
            .position(0.1) // 0.1 rad
            .build();
        let decoded = DecodedMessage::Rudder(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(127245)).unwrap();
        let values = &delta.updates[0].values;
        assert_eq!(values[0].path, "steering.rudderAngle");
        assert!((values[0].value.as_f64().unwrap() - 0.1).abs() < 1e-6);
    }

    #[test]
    fn engine_rapid_rpm_to_rad_per_s() {
        let msg = engine_parameters_rapid_update::EngineParametersRapidUpdate::builder()
            .instance_raw(0) // port / single
            .speed(3000.0) // 3000 RPM
            .build();
        let decoded = DecodedMessage::EngineParametersRapidUpdate(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(127488)).unwrap();
        let values = &delta.updates[0].values;
        let rev = values
            .iter()
            .find(|v| v.path == "propulsion.0.revolutions")
            .unwrap();
        let expected = 3000.0 * std::f64::consts::TAU / 60.0;
        assert!((rev.value.as_f64().unwrap() - expected).abs() < 0.01);
    }

    #[test]
    fn engine_dynamic_temperatures() {
        let msg = engine_parameters_dynamic::EngineParametersDynamic::builder()
            .instance_raw(1) // starboard
            .oil_pressure(300_000.0)
            .temperature(365.0) // coolant 92°C in K
            .alternator_potential(14.2)
            .build();
        let decoded = DecodedMessage::EngineParametersDynamic(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(127489)).unwrap();
        let values = &delta.updates[0].values;
        assert!(values.iter().any(|v| v.path == "propulsion.1.oilPressure"));
        assert!(
            values
                .iter()
                .any(|v| v.path == "propulsion.1.coolantTemperature")
        );
        assert!(
            values
                .iter()
                .any(|v| v.path == "propulsion.1.alternatorVoltage")
        );
    }

    #[test]
    fn battery_status() {
        let msg = battery_status::BatteryStatus::builder()
            .instance(0)
            .voltage(12.6)
            .current(-5.0)
            .temperature(298.15)
            .build();
        let decoded = DecodedMessage::BatteryStatus(msg);
        let delta = super::super::decoded_to_delta(&decoded, &test_source(127508)).unwrap();
        let values = &delta.updates[0].values;
        assert!(
            values
                .iter()
                .any(|v| v.path == "electrical.batteries.0.voltage")
        );
        assert!(
            values
                .iter()
                .any(|v| v.path == "electrical.batteries.0.current")
        );
        assert!(
            values
                .iter()
                .any(|v| v.path == "electrical.batteries.0.temperature")
        );
        let volt = values
            .iter()
            .find(|v| v.path == "electrical.batteries.0.voltage")
            .unwrap();
        assert!((volt.value.as_f64().unwrap() - 12.6).abs() < 0.01);
    }
}
