/// Calculator trait and registry for derived data computations.
///
/// Each calculator declares its input paths and produces derived output
/// values when all required inputs are available.
use signalk_types::PathValue;
use std::collections::HashMap;

pub mod air_density;
pub mod course_over_ground_magnetic;
pub mod depth_below_keel;
pub mod dew_point;
pub mod heading_true;
pub mod set_drift;
pub mod true_wind;
pub mod vmg;

/// A calculator that derives values from raw sensor data.
///
/// Calculators are pure functions: given a snapshot of current values,
/// they produce derived output values (or None if inputs are missing).
pub trait Calculator: Send + Sync {
    /// Human-readable name for config and logging.
    fn name(&self) -> &str;

    /// SignalK paths this calculator needs as input.
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

/// Create all available calculators.
pub fn all_calculators() -> Vec<Box<dyn Calculator>> {
    vec![
        Box::new(heading_true::HeadingTrue),
        Box::new(course_over_ground_magnetic::CourseOverGroundMagnetic),
        Box::new(depth_below_keel::DepthBelowKeel),
        Box::new(air_density::AirDensity),
        Box::new(dew_point::DewPoint),
        Box::new(true_wind::TrueWind),
        Box::new(vmg::VmgWind),
        Box::new(set_drift::SetDrift),
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
