/// Derives `environment.depth.belowKeel` from depth below transducer minus keel offset.
///
/// belowKeel = belowTransducer − surfaceToTransducer + design.draft
///
/// This is the "variant 2" from upstream signalk-derived-data (depthBelowKeel2.js).
/// The existing `depth_below_keel.rs` computes from surfaceToTransducer; this one
/// uses the transducer-to-keel offset directly.
///
/// transducerToKeel = surfaceToTransducer − draft
/// belowKeel = belowTransducer − transducerToKeel
///
/// Simplified: if we know surfaceToTransducer and draft, we can derive
/// the transducer-to-keel distance. But the simplest useful version is:
///
/// belowKeel = belowTransducer − (surfaceToTransducer − draft)
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct TransducerToKeel;

impl Calculator for TransducerToKeel {
    fn name(&self) -> &str {
        "transducerToKeel"
    }

    fn inputs(&self) -> &[&str] {
        &[
            "environment.depth.belowTransducer",
            "environment.depth.surfaceToTransducer",
            "design.draft.value.current",
        ]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let below_transducer = values.get("environment.depth.belowTransducer")?.as_f64()?;
        let surface_to_transducer = values
            .get("environment.depth.surfaceToTransducer")?
            .as_f64()?;
        let draft = values.get("design.draft.value.current")?.as_f64()?;

        // transducer-to-keel offset
        let transducer_to_keel = draft - surface_to_transducer;

        // depth below keel = below transducer - transducer-to-keel offset
        let below_keel = below_transducer - transducer_to_keel;

        Some(vec![
            PathValue::new(
                "environment.depth.transducerToKeel",
                serde_json::json!(transducer_to_keel),
            ),
            PathValue::new("environment.depth.belowKeel", serde_json::json!(below_keel)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_calculation() {
        let calc = TransducerToKeel;
        let mut values = HashMap::new();
        // Transducer reads 10m, transducer 0.5m below surface, draft 1.8m
        // transducerToKeel = 1.8 - 0.5 = 1.3m
        // belowKeel = 10.0 - 1.3 = 8.7m
        values.insert(
            "environment.depth.belowTransducer".into(),
            serde_json::json!(10.0),
        );
        values.insert(
            "environment.depth.surfaceToTransducer".into(),
            serde_json::json!(0.5),
        );
        values.insert("design.draft.value.current".into(), serde_json::json!(1.8));

        let result = calc.calculate(&values).unwrap();
        assert_eq!(result.len(), 2);

        let ttk = result
            .iter()
            .find(|pv| pv.path == "environment.depth.transducerToKeel")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!((ttk - 1.3).abs() < 1e-10);

        let bk = result
            .iter()
            .find(|pv| pv.path == "environment.depth.belowKeel")
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!((bk - 8.7).abs() < 1e-10);
    }
}
