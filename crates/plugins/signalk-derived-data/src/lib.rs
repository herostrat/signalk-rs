/// Derived data plugin for signalk-rs.
///
/// Subscribes to raw sensor paths and computes derived values (true heading,
/// depth below keel, air density, dew point, etc.). Each calculator is a
/// pure function that runs when its inputs change and emits results back
/// into the store via `handle_message`.
///
/// Modeled after [signalk-derived-data](https://github.com/SignalK/signalk-derived-data)
/// but implemented as a Tier 1 Rust plugin for zero-overhead, type-safe computation.
use async_trait::async_trait;
use serde::Deserialize;
use signalk_plugin_api::{
    Plugin, PluginContext, PluginError, PluginMetadata, SubscriptionHandle, SubscriptionSpec,
    delta_callback,
};
use signalk_types::{Delta, PathValue, Source, Subscription, Update};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::info;

pub mod calculators;
use calculators::Calculator;

// ─── Config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
struct DerivedDataConfig {
    /// List of calculator names to disable. All calculators are enabled by default.
    #[serde(default)]
    disabled: Vec<String>,
}

// ─── Plugin ─────────────────────────────────────────────────────────────────

pub struct DerivedDataPlugin {
    subscription_handle: Option<SubscriptionHandle>,
    ctx: Option<Arc<dyn PluginContext>>,
}

impl DerivedDataPlugin {
    pub fn new() -> Self {
        DerivedDataPlugin {
            subscription_handle: None,
            ctx: None,
        }
    }
}

impl Default for DerivedDataPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for DerivedDataPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "derived-data",
            "Derived Data",
            "Computes derived values from raw sensor data",
            "0.1.0",
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "disabled": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of calculator names to disable",
                    "default": []
                }
            }
        }))
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let derived_config: DerivedDataConfig = serde_json::from_value(config)
            .map_err(|e| PluginError::config(format!("invalid derived-data config: {e}")))?;

        // Build list of enabled calculators
        let all_calcs = calculators::all_calculators();
        let enabled: Vec<Box<dyn Calculator>> = all_calcs
            .into_iter()
            .filter(|c| !derived_config.disabled.contains(&c.name().to_string()))
            .collect();

        if enabled.is_empty() {
            ctx.set_status("No calculators enabled");
            return Ok(());
        }

        // Collect all unique input paths
        let mut input_paths: Vec<String> = Vec::new();
        for calc in &enabled {
            for path in calc.inputs() {
                let p = path.to_string();
                if !input_paths.contains(&p) {
                    input_paths.push(p);
                }
            }
        }

        let enabled_count = enabled.len();
        let calc_names: Vec<&str> = enabled.iter().map(|c| c.name()).collect();
        info!(
            calculators = ?calc_names,
            input_paths = input_paths.len(),
            "Derived data starting"
        );

        // Shared state: snapshot of latest values per path
        let snapshot: Arc<Mutex<HashMap<String, serde_json::Value>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let snapshot_clone = snapshot.clone();
        let ctx_clone = ctx.clone();

        // Subscribe to all input paths
        let subscriptions: Vec<Subscription> = input_paths.iter().map(Subscription::path).collect();

        let handle = ctx
            .subscribe(
                SubscriptionSpec::self_vessel(subscriptions),
                delta_callback(move |delta: Delta| {
                    // Update snapshot with incoming values
                    let mut changed_paths = Vec::new();
                    {
                        let mut snap = snapshot_clone.lock().unwrap();
                        for update in &delta.updates {
                            for pv in &update.values {
                                snap.insert(pv.path.clone(), pv.value.clone());
                                changed_paths.push(pv.path.clone());
                            }
                        }
                    }

                    // Run calculators whose inputs changed
                    let snap = snapshot_clone.lock().unwrap();
                    let mut derived_values: Vec<PathValue> = Vec::new();

                    for calc in &enabled {
                        // Check if any of this calculator's inputs changed
                        let affected = calc
                            .inputs()
                            .iter()
                            .any(|input| changed_paths.iter().any(|cp| cp == *input));

                        if !affected {
                            continue;
                        }

                        if let Some(outputs) = calc.calculate(&snap) {
                            derived_values.extend(outputs);
                        }
                    }
                    drop(snap);

                    // Emit derived values
                    if !derived_values.is_empty() {
                        let delta = Delta::self_vessel(vec![Update::new(
                            Source::plugin("derived-data"),
                            derived_values,
                        )]);
                        let ctx = ctx_clone.clone();
                        tokio::spawn(async move {
                            if let Err(e) = ctx.handle_message(delta).await {
                                tracing::warn!(error = %e, "Derived data: failed to emit delta");
                            }
                        });
                    }
                }),
            )
            .await?;

        self.subscription_handle = Some(handle);
        self.ctx = Some(ctx.clone());

        let status = format!("{} calculators active", enabled_count);
        ctx.set_status(&status);

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        if let (Some(handle), Some(ctx)) = (self.subscription_handle.take(), self.ctx.take()) {
            ctx.unsubscribe(handle).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_plugin_api::testing::MockPluginContext;

    #[test]
    fn metadata_id() {
        let plugin = DerivedDataPlugin::new();
        assert_eq!(plugin.metadata().id, "derived-data");
    }

    #[test]
    fn default_config_deserializes() {
        let config: DerivedDataConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(config.disabled.is_empty());
    }

    #[test]
    fn config_with_disabled() {
        let config: DerivedDataConfig = serde_json::from_value(serde_json::json!({
            "disabled": ["headingTrue", "airDensity"]
        }))
        .unwrap();
        assert_eq!(config.disabled.len(), 2);
        assert!(config.disabled.contains(&"headingTrue".to_string()));
    }

    #[tokio::test]
    async fn start_with_default_config() {
        let mut plugin = DerivedDataPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        let result = plugin.start(serde_json::json!({}), ctx.clone()).await;
        assert!(result.is_ok());

        {
            let statuses = ctx.status_messages.lock().unwrap();
            assert!(!statuses.is_empty());
            assert!(statuses[0].contains("calculators active"));
        }

        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn derives_heading_true_from_subscription() {
        let mut plugin = DerivedDataPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        plugin
            .start(serde_json::json!({}), ctx.clone())
            .await
            .unwrap();

        // Deliver a delta with heading magnetic + variation
        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![
                PathValue::new("navigation.headingMagnetic", serde_json::json!(1.5)),
                PathValue::new("navigation.magneticVariation", serde_json::json!(0.05)),
            ],
        )]);

        ctx.deliver_delta(&delta);

        // Give the spawned task time to emit
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Check that derived data was emitted
        {
            let emitted = ctx.emitted_deltas.lock().unwrap();
            let has_heading_true = emitted.iter().any(|d| {
                d.updates.iter().any(|u| {
                    u.values
                        .iter()
                        .any(|pv| pv.path == "navigation.headingTrue")
                })
            });
            assert!(
                has_heading_true,
                "Expected navigation.headingTrue in emitted deltas, got: {:?}",
                *emitted
            );
        }

        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn disabled_calculator_not_active() {
        let mut plugin = DerivedDataPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        plugin
            .start(
                serde_json::json!({ "disabled": ["headingTrue"] }),
                ctx.clone(),
            )
            .await
            .unwrap();

        // Deliver heading data
        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![
                PathValue::new("navigation.headingMagnetic", serde_json::json!(1.5)),
                PathValue::new("navigation.magneticVariation", serde_json::json!(0.05)),
            ],
        )]);

        ctx.deliver_delta(&delta);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // headingTrue should NOT be emitted (it's disabled)
        {
            let emitted = ctx.emitted_deltas.lock().unwrap();
            let has_heading_true = emitted.iter().any(|d| {
                d.updates.iter().any(|u| {
                    u.values
                        .iter()
                        .any(|pv| pv.path == "navigation.headingTrue")
                })
            });
            assert!(
                !has_heading_true,
                "headingTrue should be disabled but was emitted"
            );
        }

        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn derives_air_density() {
        let mut plugin = DerivedDataPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        plugin
            .start(serde_json::json!({}), ctx.clone())
            .await
            .unwrap();

        // Standard atmosphere
        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![
                PathValue::new("environment.outside.temperature", serde_json::json!(288.15)),
                PathValue::new("environment.outside.pressure", serde_json::json!(101325.0)),
            ],
        )]);

        ctx.deliver_delta(&delta);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        {
            let emitted = ctx.emitted_deltas.lock().unwrap();
            let density_pv = emitted.iter().find_map(|d| {
                d.updates.iter().find_map(|u| {
                    u.values
                        .iter()
                        .find(|pv| pv.path == "environment.outside.density")
                })
            });
            assert!(
                density_pv.is_some(),
                "Expected air density in emitted deltas"
            );

            let density = density_pv.unwrap().value.as_f64().unwrap();
            assert!(
                (density - 1.225).abs() < 0.01,
                "Expected ~1.225 kg/m³, got {density}"
            );
        }

        plugin.stop().await.unwrap();
    }
}
