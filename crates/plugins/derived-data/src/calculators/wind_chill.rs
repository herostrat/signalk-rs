/// Derives `environment.outside.apparentWindChillTemperature` from temperature and wind speed.
///
/// Uses the North American / UK wind chill formula (Environment Canada):
///   WC = 13.12 + 0.6215·T − 11.37·V^0.16 + 0.3965·T·V^0.16
///
/// Where T is in °C and V is wind speed in km/h.
/// Valid for T ≤ 10°C and V ≥ 4.8 km/h. Returns air temp if outside these bounds.
///
/// Uses apparent wind speed (what sensors on the boat measure).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

const KELVIN_OFFSET: f64 = 273.15;

pub struct WindChill;

impl Calculator for WindChill {
    fn name(&self) -> &str {
        "windChill"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.outside.temperature",
            "environment.wind.speedApparent",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let temp_k = values.get("environment.outside.temperature")?.as_f64()?;
        let wind_ms = values.get("environment.wind.speedApparent")?.as_f64()?;

        if temp_k <= 0.0 || wind_ms < 0.0 {
            return None;
        }

        let temp_c = temp_k - KELVIN_OFFSET;
        let wind_kmh = wind_ms * 3.6;

        let wc_c = if temp_c <= 10.0 && wind_kmh >= 4.8 {
            let v016 = wind_kmh.powf(0.16);
            13.12 + 0.6215 * temp_c - 11.37 * v016 + 0.3965 * temp_c * v016
        } else {
            temp_c
        };

        let wc_k = wc_c + KELVIN_OFFSET;

        Some(vec![PathValue::new(
            "environment.outside.apparentWindChillTemperature",
            serde_json::json!(wc_k),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_windy() {
        let calc = WindChill;
        let mut values = HashMap::new();
        // -5°C = 268.15K, 10 m/s wind (~36 km/h) → wind chill < -5°C
        values.insert(
            "environment.outside.temperature".into(),
            serde_json::json!(268.15),
        );
        values.insert(
            "environment.wind.speedApparent".into(),
            serde_json::json!(10.0),
        );
        let result = calc.calculate(&values).unwrap();
        let wc_k = result[0].value.as_f64().unwrap();
        assert!(
            wc_k < 268.15,
            "Wind chill should be below air temp, got {}K",
            wc_k
        );
    }

    #[test]
    fn warm_day_returns_air_temp() {
        let calc = WindChill;
        let mut values = HashMap::new();
        // 20°C = 293.15K → above 10°C threshold → returns air temp
        values.insert(
            "environment.outside.temperature".into(),
            serde_json::json!(293.15),
        );
        values.insert(
            "environment.wind.speedApparent".into(),
            serde_json::json!(10.0),
        );
        let result = calc.calculate(&values).unwrap();
        let wc_k = result[0].value.as_f64().unwrap();
        assert!(
            (wc_k - 293.15).abs() < 0.01,
            "Above threshold, should return air temp"
        );
    }

    #[test]
    fn light_wind_returns_air_temp() {
        let calc = WindChill;
        let mut values = HashMap::new();
        // 0°C, 1 m/s wind (3.6 km/h < 4.8 threshold)
        values.insert(
            "environment.outside.temperature".into(),
            serde_json::json!(273.15),
        );
        values.insert(
            "environment.wind.speedApparent".into(),
            serde_json::json!(1.0),
        );
        let result = calc.calculate(&values).unwrap();
        let wc_k = result[0].value.as_f64().unwrap();
        assert!(
            (wc_k - 273.15).abs() < 0.01,
            "Below wind threshold, should return air temp"
        );
    }
}
