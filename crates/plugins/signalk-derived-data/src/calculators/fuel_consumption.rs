/// Derives `propulsion.{id}.fuel.economy` from SOG and fuel rate.
///
/// economy = SOG / fuel.rate (meters per cubic meter, i.e., m/m³)
///
/// Common display unit: liters per nautical mile.
/// Conversion: m/m³ → L/NM = 1852 / (economy × 1000)
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct FuelConsumption;

impl Calculator for FuelConsumption {
    fn name(&self) -> &str {
        "fuelConsumption"
    }

    fn inputs(&self) -> &[&str] {
        &["navigation.speedOverGround", "propulsion"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let sog = values.get("navigation.speedOverGround")?.as_f64()?;

        if sog < 0.1 {
            // Too slow for meaningful economy calculation
            return None;
        }

        let mut results = Vec::new();

        for (path, value) in values.iter() {
            let Some(prefix) = path.strip_suffix(".fuel.rate") else {
                continue;
            };
            if !prefix.starts_with("propulsion.") {
                continue;
            }
            let Some(rate) = value.as_f64() else {
                continue;
            };
            if rate <= 0.0 {
                continue;
            }
            let economy = sog / rate;
            results.push(PathValue::new(
                format!("{prefix}.fuel.economy"),
                serde_json::json!(economy),
            ));
        }

        if results.is_empty() {
            None
        } else {
            Some(results)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_economy() {
        let calc = FuelConsumption;
        let mut values = HashMap::new();
        // 5 m/s SOG, 0.001 m³/s fuel rate
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));
        values.insert("propulsion.main.fuel.rate".into(), serde_json::json!(0.001));
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "propulsion.main.fuel.economy");
        let economy = result[0].value.as_f64().unwrap();
        assert!((economy - 5000.0).abs() < 0.01);
    }

    #[test]
    fn too_slow() {
        let calc = FuelConsumption;
        let mut values = HashMap::new();
        values.insert("navigation.speedOverGround".into(), serde_json::json!(0.01));
        values.insert("propulsion.main.fuel.rate".into(), serde_json::json!(0.001));
        assert!(calc.calculate(&values).is_none());
    }

    #[test]
    fn zero_fuel_rate_skipped() {
        let calc = FuelConsumption;
        let mut values = HashMap::new();
        values.insert("navigation.speedOverGround".into(), serde_json::json!(5.0));
        values.insert("propulsion.main.fuel.rate".into(), serde_json::json!(0.0));
        assert!(calc.calculate(&values).is_none());
    }
}
