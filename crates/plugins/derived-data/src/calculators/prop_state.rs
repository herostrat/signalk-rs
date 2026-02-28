/// Derives `propulsion.{id}.state.value` from revolutions.
///
/// If revolutions > 0 → "started", else "stopped".
/// Iterates over all propulsion instances found in the snapshot.
use super::Calculator;
use signalk_types::PathValue;
use std::collections::HashMap;

pub struct PropState;

impl Calculator for PropState {
    fn name(&self) -> &str {
        "propState"
    }

    fn inputs(&self) -> &[&str] {
        &["propulsion"]
    }

    fn calculate(&self, values: &HashMap<String, serde_json::Value>) -> Option<Vec<PathValue>> {
        let mut results = Vec::new();

        for (path, value) in values.iter() {
            let Some(prefix) = path.strip_suffix(".revolutions") else {
                continue;
            };
            if !prefix.starts_with("propulsion.") {
                continue;
            }
            let Some(rpm) = value.as_f64() else {
                continue;
            };
            let state = if rpm > 0.0 { "started" } else { "stopped" };
            results.push(PathValue::new(
                format!("{prefix}.state.value"),
                serde_json::json!(state),
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
    fn engine_running() {
        let calc = PropState;
        let mut values = HashMap::new();
        values.insert(
            "propulsion.main.revolutions".into(),
            serde_json::json!(50.0),
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].path, "propulsion.main.state.value");
        assert_eq!(result[0].value.as_str().unwrap(), "started");
    }

    #[test]
    fn engine_stopped() {
        let calc = PropState;
        let mut values = HashMap::new();
        values.insert("propulsion.main.revolutions".into(), serde_json::json!(0.0));
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result[0].value.as_str().unwrap(), "stopped");
    }

    #[test]
    fn multiple_engines() {
        let calc = PropState;
        let mut values = HashMap::new();
        values.insert(
            "propulsion.port.revolutions".into(),
            serde_json::json!(40.0),
        );
        values.insert(
            "propulsion.starboard.revolutions".into(),
            serde_json::json!(0.0),
        );
        let result = calc.calculate(&values).unwrap();
        assert_eq!(result.len(), 2);
    }
}
