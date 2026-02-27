# Writing a Rust Plugin

This guide walks through creating a Tier 1 (in-process) Rust plugin for signalk-rs.

## Prerequisites

- Rust toolchain (edition 2024)
- signalk-rs workspace checked out

## Step 1: Create the Crate

```bash
mkdir -p crates/plugins/signalk-my-plugin/src
```

`crates/plugins/signalk-my-plugin/Cargo.toml`:

```toml
[package]
name = "signalk-plugin-my-plugin"
version.workspace = true
edition.workspace = true

[dependencies]
signalk-plugin-api.workspace = true
signalk-types.workspace = true
tokio.workspace = true
tracing.workspace = true
serde_json.workspace = true
async-trait.workspace = true
```

Add to root `Cargo.toml`:

```toml
# [workspace] members
"crates/plugins/signalk-my-plugin",

# [workspace.dependencies]
signalk-plugin-my-plugin = { path = "crates/plugins/signalk-my-plugin" }
```

## Step 2: Implement the Plugin Trait

`src/lib.rs`:

```rust
use async_trait::async_trait;
use signalk_plugin_api::{Plugin, PluginContext, PluginError, PluginMetadata};
use std::sync::Arc;

pub struct MyPlugin;

impl MyPlugin {
    pub fn new() -> Self { MyPlugin }
}

#[async_trait]
impl Plugin for MyPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "my-plugin",           // id (must be unique)
            "My Plugin",           // display name
            "Does something cool", // description
            "0.1.0",               // version
        )
    }

    fn schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "interval_secs": {
                    "type": "integer",
                    "description": "Check interval in seconds",
                    "default": 30
                }
            }
        }))
    }

    async fn start(
        &mut self,
        config: serde_json::Value,
        ctx: Arc<dyn PluginContext>,
    ) -> Result<(), PluginError> {
        let interval = config["interval_secs"].as_u64().unwrap_or(30);
        ctx.set_status(&format!("Running, interval: {interval}s"));
        // Start your background work here...
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}
```

## Step 3: Register in the Server

In `crates/signalk-server/Cargo.toml`, add:

```toml
signalk-plugin-my-plugin.workspace = true
```

In `crates/signalk-server/src/main.rs`, register:

```rust
plugin_manager.register(Box::new(
    signalk_plugin_my_plugin::MyPlugin::new(),
));
```

## Step 4: Configure

In `signalk-rs.toml`:

```toml
[[plugins]]
id = "my-plugin"
config = { interval_secs = 15 }
```

## Plugin API Reference

### Reading Data

```rust
// Read a value from the self vessel
let speed = ctx.get_self_path("navigation.speedOverGround").await?;

// Read from any vessel
let value = ctx.get_path("vessels.urn:mrn:signalk:uuid:xxx.navigation.position").await?;
```

### Writing Data (Deltas)

```rust
use signalk_types::{Delta, PathValue, Source, Update};

let delta = Delta::self_vessel(vec![Update::new(
    Source::plugin("my-plugin"),
    vec![PathValue::new("environment.wind.speedApparent", serde_json::json!(12.5))],
)]);

ctx.handle_message(delta).await?;
```

### Subscriptions

```rust
use signalk_plugin_api::{SubscriptionSpec, delta_callback};
use signalk_types::Subscription;

let handle = ctx.subscribe(
    SubscriptionSpec::self_vessel(vec![
        Subscription::path("navigation.position"),
        Subscription::path("navigation.speedOverGround"),
    ]),
    delta_callback(move |delta| {
        // Called on every matching delta
        for update in &delta.updates {
            for pv in &update.values {
                tracing::info!(path = %pv.path, "Got update");
            }
        }
    }),
).await?;

// Later, unsubscribe:
ctx.unsubscribe(handle).await?;
```

### PUT Handlers

```rust
use signalk_plugin_api::{put_handler, PutHandlerResult};

ctx.register_put_handler(
    "vessels.self",
    "steering.autopilot.target.headingTrue",
    put_handler(|cmd| async move {
        tracing::info!(path = %cmd.path, value = %cmd.value, "PUT received");
        // Do something with the command...
        Ok(PutHandlerResult::Completed)
    }),
).await?;
```

### REST Endpoints

```rust
use signalk_plugin_api::{route_handler, PluginRequest, PluginResponse};

ctx.register_routes(Box::new(|router| {
    router.get("/status", route_handler(|_req| async {
        Ok(PluginResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: serde_json::json!({"status": "ok"}).to_string().into_bytes(),
        })
    }));
})).await?;
// Accessible at: GET /plugins/my-plugin/status
```

### Status Reporting

```rust
ctx.set_status("Connected, processing 47 sentences/s");
ctx.set_error("Connection lost, retrying in 5s");
```

## Testing with MockPluginContext

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use signalk_plugin_api::testing::MockPluginContext;

    #[tokio::test]
    async fn plugin_starts_successfully() {
        let mut plugin = MyPlugin::new();
        let ctx = Arc::new(MockPluginContext::new());

        let result = plugin.start(serde_json::json!({}), ctx.clone()).await;
        assert!(result.is_ok());

        // Check status was set
        let statuses = ctx.status_messages.lock().unwrap();
        assert!(!statuses.is_empty());
    }
}
```

The `MockPluginContext` records all interactions:
- `emitted_deltas` — deltas sent via `handle_message`
- `registered_put_paths` — paths registered via `register_put_handler`
- `status_messages` / `error_messages` — status updates
- `saved_options` — config saved via `save_options`
- `stored_values` — pre-seeded values for `get_self_path`
- `deliver_delta()` — simulate incoming deltas to subscriptions

## Best Practices

1. **Only depend on `signalk-plugin-api` and `signalk-types`** — never on server internals
2. **Use `ctx.set_status()`** to report plugin health
3. **Use `ctx.set_error()`** on recoverable errors (the plugin stays running)
4. **Return `PluginError`** from `start()` on fatal config errors (plugin won't start)
5. **Spawn background tasks inside `start()`** — keep `start()` fast
6. **Store abort handles** for cleanup in `stop()`
7. **Test with `MockPluginContext`** — no server needed
