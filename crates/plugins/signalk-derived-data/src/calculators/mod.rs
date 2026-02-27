/// Calculator trait and registry for derived data computations.
///
/// Each calculator declares its input paths and produces derived output
/// values when all required inputs are available.
use signalk_types::PathValue;
use std::collections::HashMap;

pub mod air_density;
pub mod battery_power;
pub mod course_over_ground_magnetic;
pub mod course_over_ground_true;
pub mod depth_below_keel;
pub mod depth_below_surface;
pub mod dew_point;
pub mod eta;
pub mod fuel_consumption;
pub mod heading_true;
pub mod heat_index;
pub mod leeway;
pub mod leeway_angle;
pub mod moon;
pub mod prop_slip;
pub mod prop_state;
pub mod set_drift;
pub mod steer_error;
pub mod suncalc;
pub mod suntime;
pub mod tank_volume;
pub mod transducer_to_keel;
pub mod true_wind;
pub mod vmg;
pub mod vmg_stw;
pub mod wind_chill;
pub mod wind_direction_magnetic;
pub mod wind_direction_magnetic2;
pub mod wind_ground;
pub mod wind_ground_direction;
pub mod wind_shift;

/// A calculator that derives values from raw sensor data.
///
/// Calculators are pure functions: given a snapshot of current values,
/// they produce derived output values (or None if inputs are missing).
pub trait Calculator: Send + Sync {
    /// Human-readable name for config and logging.
    fn name(&self) -> &str;

    /// SignalK paths this calculator needs as input.
    ///
    /// For static calculators, these are exact paths (e.g. "navigation.headingMagnetic").
    /// For dynamic-instance calculators, these are path prefixes (e.g. "electrical.batteries")
    /// and the plugin subscribes to all sub-paths via wildcard.
    fn inputs(&self) -> &[&str];

    /// Compute derived values from the current input snapshot.
    ///
    /// Returns `None` if required inputs are missing or invalid.
    /// Returns `Some(vec![...])` with one or more derived path-value pairs.
    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>>;
}

/// Normalize an angle to [0, 2π).
pub fn normalize_angle(angle: f64) -> f64 {
    angle.rem_euclid(2.0 * std::f64::consts::PI)
}

/// Check if a changed path matches a calculator's input (exact or prefix).
pub fn path_matches_input(changed_path: &str, input: &str) -> bool {
    changed_path == input || changed_path.starts_with(&format!("{input}."))
}

/// Create all available calculators.
pub fn all_calculators() -> Vec<Box<dyn Calculator>> {
    vec![
        Box::new(heading_true::HeadingTrue),
        Box::new(course_over_ground_magnetic::CourseOverGroundMagnetic),
        Box::new(course_over_ground_true::CourseOverGroundTrue),
        Box::new(depth_below_keel::DepthBelowKeel),
        Box::new(depth_below_surface::DepthBelowSurface),
        Box::new(transducer_to_keel::TransducerToKeel),
        Box::new(air_density::AirDensity),
        Box::new(dew_point::DewPoint),
        Box::new(heat_index::HeatIndex),
        Box::new(wind_chill::WindChill),
        Box::new(true_wind::TrueWind),
        Box::new(vmg::VmgWind),
        Box::new(vmg_stw::VmgStw),
        Box::new(leeway_angle::LeewayAngle),
        Box::new(set_drift::SetDrift),
        Box::new(wind_direction_magnetic::WindDirectionMagnetic),
        Box::new(wind_direction_magnetic2::WindDirectionMagnetic2),
        Box::new(wind_ground::WindGround),
        Box::new(wind_ground_direction::WindGroundDirection),
        Box::new(wind_shift::WindShift::new()),
        Box::new(leeway::Leeway),
        Box::new(eta::Eta),
        Box::new(steer_error::SteerError),
        // Astronomical calculators
        Box::new(suncalc::SunCalc),
        Box::new(suntime::SunTime),
        Box::new(moon::Moon),
        // Dynamic-instance calculators (prefix-based inputs)
        Box::new(battery_power::BatteryPower),
        Box::new(prop_state::PropState),
        Box::new(fuel_consumption::FuelConsumption),
        Box::new(prop_slip::PropSlip),
        Box::new(tank_volume::TankVolume),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_calculators_have_names_and_inputs() {
        for calc in all_calculators() {
            assert!(!calc.name().is_empty(), "Calculator has empty name");
            assert!(
                !calc.inputs().is_empty(),
                "Calculator {} has no inputs",
                calc.name()
            );
        }
    }

    #[test]
    fn all_calculators_return_none_for_empty_inputs() {
        let empty = HashMap::new();
        for calc in all_calculators() {
            assert!(
                calc.calculate(&empty).is_none(),
                "Calculator {} should return None for empty inputs",
                calc.name()
            );
        }
    }
}
