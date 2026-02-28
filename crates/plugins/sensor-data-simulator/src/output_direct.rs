/// Direct SignalK delta output — converts SimulatedValues to Delta and emits via PluginContext.
use crate::generators::SimulatedValues;
use signalk_types::{Delta, PathValue, Source, Update};

pub fn build_delta(values: &SimulatedValues, enable_environment: bool) -> Delta {
    let source = Source::plugin("sensor-data-simulator");
    let mut path_values = Vec::with_capacity(30);

    // Navigation (always included)
    path_values.push(PathValue::new(
        "navigation.position",
        serde_json::json!({
            "latitude": values.latitude,
            "longitude": values.longitude
        }),
    ));
    path_values.push(PathValue::new(
        "navigation.speedOverGround",
        serde_json::json!(values.sog_mps),
    ));
    path_values.push(PathValue::new(
        "navigation.courseOverGroundTrue",
        serde_json::json!(values.cog_rad),
    ));
    path_values.push(PathValue::new(
        "navigation.headingMagnetic",
        serde_json::json!(values.heading_magnetic_rad),
    ));
    path_values.push(PathValue::new(
        "navigation.magneticVariation",
        serde_json::json!(values.magnetic_variation_rad),
    ));
    path_values.push(PathValue::new(
        "navigation.speedThroughWater",
        serde_json::json!(values.stw_mps),
    ));
    path_values.push(PathValue::new(
        "navigation.attitude",
        serde_json::json!({
            "roll": values.roll_rad,
            "pitch": values.pitch_rad,
            "yaw": 0.0
        }),
    ));
    path_values.push(PathValue::new(
        "navigation.datetime",
        serde_json::json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
    ));

    // Environment (optional)
    if enable_environment {
        path_values.push(PathValue::new(
            "environment.wind.angleApparent",
            serde_json::json!(values.wind_angle_apparent_rad),
        ));
        path_values.push(PathValue::new(
            "environment.wind.speedApparent",
            serde_json::json!(values.wind_speed_apparent_mps),
        ));
        path_values.push(PathValue::new(
            "environment.depth.belowTransducer",
            serde_json::json!(values.depth_below_transducer_m),
        ));
        path_values.push(PathValue::new(
            "environment.depth.surfaceToTransducer",
            serde_json::json!(values.surface_to_transducer_m),
        ));
        path_values.push(PathValue::new(
            "environment.water.temperature",
            serde_json::json!(values.water_temperature_k),
        ));
        path_values.push(PathValue::new(
            "environment.outside.temperature",
            serde_json::json!(values.air_temperature_k),
        ));
        path_values.push(PathValue::new(
            "environment.outside.pressure",
            serde_json::json!(values.pressure_pa),
        ));
        path_values.push(PathValue::new(
            "environment.outside.humidity",
            serde_json::json!(values.humidity_ratio),
        ));
    }

    // Propulsion + electrical + fuel (optional)
    if let Some(ref prop) = values.propulsion {
        path_values.push(PathValue::new(
            "propulsion.main.revolutions",
            serde_json::json!(prop.revolutions_hz),
        ));
        path_values.push(PathValue::new(
            "propulsion.main.oilTemperature",
            serde_json::json!(prop.oil_temperature_k),
        ));
        path_values.push(PathValue::new(
            "propulsion.main.coolantTemperature",
            serde_json::json!(prop.coolant_temperature_k),
        ));
        path_values.push(PathValue::new(
            "propulsion.main.fuel.rate",
            serde_json::json!(prop.fuel_rate_m3s),
        ));
        path_values.push(PathValue::new(
            "electrical.batteries.0.voltage",
            serde_json::json!(prop.battery_voltage),
        ));
        path_values.push(PathValue::new(
            "electrical.batteries.0.current",
            serde_json::json!(prop.battery_current),
        ));
    }

    // Tanks (always included)
    path_values.push(PathValue::new(
        "tanks.fuel.0.currentLevel",
        serde_json::json!(values.fuel_tank_level),
    ));
    path_values.push(PathValue::new(
        "tanks.fuel.0.capacity",
        serde_json::json!(values.fuel_tank_capacity_m3),
    ));
    path_values.push(PathValue::new(
        "tanks.freshWater.0.currentLevel",
        serde_json::json!(values.fresh_water_level),
    ));
    path_values.push(PathValue::new(
        "tanks.freshWater.0.capacity",
        serde_json::json!(values.fresh_water_capacity_m3),
    ));

    Delta::self_vessel(vec![Update::new(source, path_values)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators;

    #[test]
    fn build_delta_contains_navigation() {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, false);
        let values = state.tick();
        let delta = build_delta(&values, false);

        let paths: Vec<&str> = delta.updates[0]
            .values
            .iter()
            .map(|pv| pv.path.as_str())
            .collect();

        assert!(paths.contains(&"navigation.position"));
        assert!(paths.contains(&"navigation.speedOverGround"));
        assert!(paths.contains(&"navigation.speedThroughWater"));
        assert!(paths.contains(&"navigation.courseOverGroundTrue"));
        assert!(paths.contains(&"navigation.headingMagnetic"));
        assert!(paths.contains(&"navigation.magneticVariation"));
        assert!(paths.contains(&"navigation.attitude"));
        assert!(paths.contains(&"navigation.datetime"));
        // No environment or propulsion (but tanks are always present)
        assert!(!paths.iter().any(|p| p.starts_with("environment.")));
        assert!(!paths.iter().any(|p| p.starts_with("propulsion.")));
        assert!(paths.contains(&"tanks.fuel.0.currentLevel"));
    }

    #[test]
    fn build_delta_with_environment() {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, false);
        let values = state.tick();
        let delta = build_delta(&values, true);

        let paths: Vec<&str> = delta.updates[0]
            .values
            .iter()
            .map(|pv| pv.path.as_str())
            .collect();

        assert!(paths.contains(&"environment.wind.angleApparent"));
        assert!(paths.contains(&"environment.wind.speedApparent"));
        assert!(paths.contains(&"environment.depth.belowTransducer"));
        assert!(paths.contains(&"environment.depth.surfaceToTransducer"));
        assert!(paths.contains(&"environment.water.temperature"));
        assert!(paths.contains(&"environment.outside.temperature"));
        assert!(paths.contains(&"environment.outside.pressure"));
        assert!(paths.contains(&"environment.outside.humidity"));
    }

    #[test]
    fn build_delta_with_propulsion() {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, true);
        let values = state.tick();
        let delta = build_delta(&values, false);

        let paths: Vec<&str> = delta.updates[0]
            .values
            .iter()
            .map(|pv| pv.path.as_str())
            .collect();

        assert!(paths.contains(&"propulsion.main.revolutions"));
        assert!(paths.contains(&"propulsion.main.oilTemperature"));
        assert!(paths.contains(&"propulsion.main.coolantTemperature"));
        assert!(paths.contains(&"propulsion.main.fuel.rate"));
        assert!(paths.contains(&"electrical.batteries.0.voltage"));
        assert!(paths.contains(&"electrical.batteries.0.current"));
    }

    #[test]
    fn build_delta_position_has_lat_lon() {
        let state = generators::SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, false);
        let values = state.tick();
        let delta = build_delta(&values, false);

        let pos_pv = delta.updates[0]
            .values
            .iter()
            .find(|pv| pv.path == "navigation.position")
            .unwrap();

        assert!(pos_pv.value.get("latitude").is_some());
        assert!(pos_pv.value.get("longitude").is_some());
    }
}
