/// Derives `environment.outside.airDensity` from temperature, pressure, and humidity.
///
/// Uses the moist air density formula:
///   Ps = 611.21 · exp((18.678 − Tc/234.5) · Tc/(257.14 + Tc))  (Buck equation)
///   Pv = humidity · Ps  (actual vapor pressure)
///   Pd = P − Pv  (partial pressure dry air)
///   ρ = Pd/(Rd·T) + Pv/(Rv·T)
///
/// If humidity is not available, falls back to dry air: ρ = P/(Rd·T).
///
/// Matches upstream signalk-derived-data airDensity.js.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

/// Specific gas constant for dry air in J/(kg·K)
const R_DRY_AIR: f64 = 287.058;
/// Specific gas constant for water vapor in J/(kg·K)
const R_VAPOR: f64 = 461.495;
const KELVIN_OFFSET: f64 = 273.15;

pub struct AirDensity;

impl Calculator for AirDensity {
    fn name(&self) -> &str {
        "airDensity"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.outside.temperature",
            "environment.outside.pressure",
            "environment.outside.humidity",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let temperature_k = values.get("environment.outside.temperature")?.as_f64()?;
        let pressure_pa = values.get("environment.outside.pressure")?.as_f64()?;

        if temperature_k <= 0.0 || pressure_pa <= 0.0 {
            return None;
        }

        let density = if let Some(humidity) = values
            .get("environment.outside.humidity")
            .and_then(|v| v.as_f64())
        {
            if humidity > 0.0 && humidity <= 1.0 {
                // Moist air density (Buck equation for saturation vapor pressure)
                let temp_c = temperature_k - KELVIN_OFFSET;
                let ps = 611.21 * ((18.678 - temp_c / 234.5) * temp_c / (257.14 + temp_c)).exp();
                let pv = humidity * ps;
                let pd = pressure_pa - pv;
                pd / (R_DRY_AIR * temperature_k) + pv / (R_VAPOR * temperature_k)
            } else {
                // Invalid humidity — fall back to dry air
                pressure_pa / (R_DRY_AIR * temperature_k)
            }
        } else {
            // No humidity — dry air approximation
            pressure_pa / (R_DRY_AIR * temperature_k)
        };

        Some(vec![PathValue::new(
            "environment.outside.airDensity",
            serde_json::json!(density),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_air_density_at_sea_level() {
        let calc = AirDensity;
        let mut values = HashMap::new();
        // Standard atmosphere: 101325 Pa, 288.15 K (15°C), no humidity
        values.insert(
            "environment.outside.temperature".to_string(),
            serde_json::json!(288.15),
        );
        values.insert(
            "environment.outside.pressure".to_string(),
            serde_json::json!(101325.0),
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "environment.outside.airDensity");
        let density = result[0].value.as_f64().unwrap();
        // Dry air at standard conditions ≈ 1.225 kg/m³
        assert!(
            (density - 1.225).abs() < 0.01,
            "Expected ~1.225 kg/m³, got {density}"
        );
    }

    #[test]
    fn moist_air_density_less_than_dry() {
        let calc = AirDensity;

        // Dry air
        let mut dry = HashMap::new();
        dry.insert(
            "environment.outside.temperature".to_string(),
            serde_json::json!(293.15), // 20°C
        );
        dry.insert(
            "environment.outside.pressure".to_string(),
            serde_json::json!(101325.0),
        );
        let dry_density = calc.calculate(&dry).unwrap()[0].value.as_f64().unwrap();

        // Moist air (80% humidity)
        let mut moist = dry.clone();
        moist.insert(
            "environment.outside.humidity".to_string(),
            serde_json::json!(0.8),
        );
        let moist_density = calc.calculate(&moist).unwrap()[0].value.as_f64().unwrap();

        // Moist air is always lighter than dry air at same T and P
        assert!(
            moist_density < dry_density,
            "Moist air ({moist_density}) should be lighter than dry ({dry_density})"
        );
        // Difference should be small but measurable
        assert!(
            (dry_density - moist_density) < 0.02,
            "Difference too large: {}",
            dry_density - moist_density
        );
    }

    #[test]
    fn rejects_zero_temp() {
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
    fn missing_pressure() {
        let calc = AirDensity;
        let mut values = HashMap::new();
        values.insert(
            "environment.outside.temperature".to_string(),
            serde_json::json!(288.15),
        );
        assert!(calc.calculate(&values).is_none());
    }
}
