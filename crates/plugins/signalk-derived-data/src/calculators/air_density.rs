/// Derives `environment.outside.density` from temperature and pressure.
///
/// Uses the ideal gas law for dry air:
///   ρ = p / (R_specific · T)
///
/// Where:
/// - p = pressure in Pa
/// - T = temperature in Kelvin
/// - R_specific = 287.058 J/(kg·K) for dry air
///
/// This is a simplified model (ignores humidity). Good enough for
/// marine weather applications.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

/// Specific gas constant for dry air in J/(kg·K)
const R_DRY_AIR: f64 = 287.058;

pub struct AirDensity;

impl Calculator for AirDensity {
    fn name(&self) -> &str {
        "airDensity"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.outside.temperature",
            "environment.outside.pressure",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let temperature_k = values.get("environment.outside.temperature")?.as_f64()?;
        let pressure_pa = values.get("environment.outside.pressure")?.as_f64()?;

        // Sanity checks
        if temperature_k <= 0.0 || pressure_pa <= 0.0 {
            return None;
        }

        let density = pressure_pa / (R_DRY_AIR * temperature_k);

        Some(vec![PathValue::new(
            "environment.outside.density",
            serde_json::json!(density),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_density_at_sea_level() {
        let calc = AirDensity;
        let mut values = HashMap::new();
        // Standard atmosphere: 101325 Pa, 288.15 K (15°C)
        values.insert(
            "environment.outside.temperature".to_string(),
            serde_json::json!(288.15),
        );
        values.insert(
            "environment.outside.pressure".to_string(),
            serde_json::json!(101325.0),
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "environment.outside.density");
        let density = result[0].value.as_f64().unwrap();
        // Standard sea-level air density ≈ 1.225 kg/m³
        assert!(
            (density - 1.225).abs() < 0.01,
            "Expected ~1.225 kg/m³, got {density}"
        );
    }

    #[test]
    fn air_density_rejects_zero_temp() {
        let calc = AirDensity;
        let mut values = HashMap::new();
        values.insert(
            "environment.outside.temperature".to_string(),
            serde_json::json!(0.0),
        );
        values.insert(
            "environment.outside.pressure".to_string(),
            serde_json::json!(101325.0),
        );
        assert!(calc.calculate(&values).is_none());
    }

    #[test]
    fn air_density_missing_pressure() {
        let calc = AirDensity;
        let mut values = HashMap::new();
        values.insert(
            "environment.outside.temperature".to_string(),
            serde_json::json!(288.15),
        );
        assert!(calc.calculate(&values).is_none());
    }
}
