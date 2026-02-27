/// Derives `environment.depth.belowSurface` from depth below keel + vessel draft.
///
/// belowSurface = belowKeel + design.draft.value
///
/// Design draft is a vessel configuration value (static).
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct DepthBelowSurface;

impl Calculator for DepthBelowSurface {
    fn name(&self) -> &str {
        "depthBelowSurface"
    }

    fn inputs(&self) -> &[&str] {
        &["environment.depth.belowKeel", "design.draft.value.current"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let below_keel = values.get("environment.depth.belowKeel")?.as_f64()?;
        let draft = values.get("design.draft.value.current")?.as_f64()?;

        if below_keel < 0.0 || draft < 0.0 {
            return None;
        }

        let below_surface = below_keel + draft;

        Some(vec![PathValue::new(
            "environment.depth.belowSurface",
            serde_json::json!(below_surface),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_calculation() {
        let calc = DepthBelowSurface;
        let mut values = HashMap::new();
        values.insert("environment.depth.belowKeel".into(), serde_json::json!(8.0));
        values.insert("design.draft.value.current".into(), serde_json::json!(1.5));
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "environment.depth.belowSurface");
        let depth = result[0].value.as_f64().unwrap();
        assert!((depth - 9.5).abs() < 1e-10);
    }

    #[test]
    fn missing_draft() {
        let calc = DepthBelowSurface;
        let mut values = HashMap::new();
        values.insert("environment.depth.belowKeel".into(), serde_json::json!(8.0));
        assert!(calc.calculate(&values).is_none());
    }
}
