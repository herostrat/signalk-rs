/// Derives `environment.depth.belowKeel` from transducer depth - draft.
///
/// Formula: belowKeel = belowTransducer - (draft - transducerDepth)
///
/// In practice, if the transducer is mounted at the hull bottom,
/// belowKeel ≈ belowTransducer - keel extension below hull.
/// Simplified: belowKeel = belowTransducer + design.draft.value
/// (design.draft.value is typically negative, representing keel depth below waterline)
///
/// We use the SignalK convention: `environment.depth.transducerToKeel` is the
/// offset from transducer to keel (positive means transducer is above keel).
/// belowKeel = belowTransducer - transducerToKeel
///
/// If transducerToKeel is not available, falls back to:
/// belowKeel = belowTransducer (assuming transducer at keel level)
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct DepthBelowKeel;

impl Calculator for DepthBelowKeel {
    fn name(&self) -> &str {
        "depthBelowKeel"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.depth.belowTransducer",
            "environment.depth.transducerToKeel",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let below_transducer = values.get("environment.depth.belowTransducer")?.as_f64()?;

        // transducerToKeel is optional — default to 0 if not available
        let transducer_to_keel = values
            .get("environment.depth.transducerToKeel")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let below_keel = below_transducer - transducer_to_keel;

        Some(vec![PathValue::new(
            "environment.depth.belowKeel",
            serde_json::json!(below_keel),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_below_keel_with_offset() {
        let calc = DepthBelowKeel;
        let mut values = HashMap::new();
        values.insert(
            "environment.depth.belowTransducer".to_string(),
            serde_json::json!(10.0),
        );
        values.insert(
            "environment.depth.transducerToKeel".to_string(),
            serde_json::json!(0.5), // transducer 0.5m above keel
        );

        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "environment.depth.belowKeel");
        let value = result[0].value.as_f64().unwrap();
        assert!((value - 9.5).abs() < 0.001);
    }

    #[test]
    fn depth_below_keel_without_offset() {
        let calc = DepthBelowKeel;
        let mut values = HashMap::new();
        values.insert(
            "environment.depth.belowTransducer".to_string(),
            serde_json::json!(10.0),
        );
        // No transducerToKeel → defaults to 0

        let result = calc.calculate(&values).unwrap();
        let value = result[0].value.as_f64().unwrap();
        assert!((value - 10.0).abs() < 0.001);
    }

    #[test]
    fn depth_missing_transducer() {
        let calc = DepthBelowKeel;
        let values = HashMap::new();
        assert!(calc.calculate(&values).is_none());
    }
}
