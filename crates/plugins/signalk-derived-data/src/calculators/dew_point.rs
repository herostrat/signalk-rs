/// Derives `environment.outside.dewPointTemperature` from temperature and humidity.
///
/// Uses the Magnus formula (August-Roche-Magnus approximation):
///   γ(T,RH) = ln(RH) + (b·T)/(c+T)
///   Td = (c · γ) / (b - γ)
///
/// Where:
/// - T = temperature in °C (converted from Kelvin input)
/// - RH = relative humidity as ratio (0.0–1.0)
/// - b = 17.67
/// - c = 243.5 °C
/// - Td = dew point in °C (converted back to Kelvin for output)
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

const B: f64 = 17.67;
const C: f64 = 243.5;
const KELVIN_OFFSET: f64 = 273.15;

pub struct DewPoint;

impl Calculator for DewPoint {
    fn name(&self) -> &str {
        "dewPoint"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.outside.temperature",
            "environment.outside.humidity",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let temp_k = values.get("environment.outside.temperature")?.as_f64()?;
        let humidity = values.get("environment.outside.humidity")?.as_f64()?;

        // Sanity checks
        if humidity <= 0.0 || humidity > 1.0 || temp_k <= 0.0 {
            return None;
        }

        let temp_c = temp_k - KELVIN_OFFSET;
        let gamma = humidity.ln() + (B * temp_c) / (C + temp_c);
        let dew_point_c = (C * gamma) / (B - gamma);
        let dew_point_k = dew_point_c + KELVIN_OFFSET;

        Some(vec![PathValue::new(
            "environment.outside.dewPointTemperature",
            serde_json::json!(dew_point_k),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dew_point_at_50_percent_humidity() {
        let calc = DewPoint;
        let mut values = HashMap::new();
        // 20°C = 293.15K, 50% humidity → dew point ≈ 9.3°C ≈ 282.45K
        values.insert(
            "environment.outside.temperature".to_string(),
            serde_json::json!(293.15),
        );
        values.insert(
            "environment.outside.humidity".to_string(),
            serde_json::json!(0.5),
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "environment.outside.dewPointTemperature");
        let dew_k = result[0].value.as_f64().unwrap();
        let dew_c = dew_k - 273.15;
        // Expected ~9.3°C
        assert!(
            (dew_c - 9.3).abs() < 1.0,
            "Expected dew point ~9.3°C, got {dew_c}°C"
        );
    }

    #[test]
    fn dew_point_at_100_percent() {
        let calc = DewPoint;
        let mut values = HashMap::new();
        // At 100% humidity, dew point ≈ air temperature
        values.insert(
            "environment.outside.temperature".to_string(),
            serde_json::json!(293.15),
        );
        values.insert(
            "environment.outside.humidity".to_string(),
            serde_json::json!(1.0),
        );

        let result = calc.calculate(&values).unwrap();
        let dew_k = result[0].value.as_f64().unwrap();
        assert!(
            (dew_k - 293.15).abs() < 0.5,
            "At 100% RH, dew point should ≈ air temp, got {dew_k}"
        );
    }

    #[test]
    fn dew_point_rejects_zero_humidity() {
        let calc = DewPoint;
        let mut values = HashMap::new();
        values.insert(
            "environment.outside.temperature".to_string(),
            serde_json::json!(293.15),
        );
        values.insert(
            "environment.outside.humidity".to_string(),
            serde_json::json!(0.0),
        );
        assert!(calc.calculate(&values).is_none());
    }
}
