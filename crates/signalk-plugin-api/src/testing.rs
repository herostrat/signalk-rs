/// Mock implementation of `PluginContext` for testing plugins.
///
/// All interactions are recorded and can be inspected after the test.
/// Pre-seed `stored_values` to control what `get_self_path` returns.
///
/// # Example
///
/// ```rust,ignore
/// use signalk_plugin_api::testing::MockPluginContext;
/// use signalk_plugin_api::{Plugin, PluginContext};
/// use std::sync::Arc;
///
/// let mock = MockPluginContext::new();
/// let ctx: Arc<dyn PluginContext> = Arc::new(mock.clone());
///
/// let mut plugin = MyPlugin::new();
/// plugin.start(serde_json::json!({}), ctx).await.unwrap();
///
/// assert_eq!(mock.status_messages.lock().unwrap().last().unwrap(), "Running");
/// ```
use async_trait::async_trait;
use signalk_types::Delta;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::context::{
    DeltaCallback, DeltaInputHandler, PluginContext, PutHandler, RouterSetup, SubscriptionHandle,
    SubscriptionSpec,
};
use crate::error::PluginError;

/// A mock `PluginContext` that records all interactions for assertion.
#[derive(Clone)]
pub struct MockPluginContext {
    /// Deltas emitted via `handle_message`.
    pub emitted_deltas: Arc<Mutex<Vec<Delta>>>,

    /// PUT handler registrations as `(context, path)` pairs.
    pub registered_put_paths: Arc<Mutex<Vec<(String, String)>>>,

    /// Status messages set via `set_status`.
    pub status_messages: Arc<Mutex<Vec<String>>>,

    /// Error messages set via `set_error`.
    pub error_messages: Arc<Mutex<Vec<String>>>,

    /// Pre-seeded path → value map for `get_self_path` responses.
    /// Also receives values written via `handle_message` (simplified: last value per path).
    pub stored_values: Arc<Mutex<HashMap<String, serde_json::Value>>>,

    /// Options saved via `save_options`.
    pub saved_options: Arc<Mutex<Option<serde_json::Value>>>,

    /// Active subscription callbacks (called when you manually inject deltas).
    pub(crate) subscriptions: Arc<Mutex<Vec<(u64, DeltaCallback)>>>,

    /// Counter for subscription handle IDs.
    pub(crate) next_sub_id: Arc<Mutex<u64>>,

    /// Delta input handlers registered via `register_delta_input_handler`.
    pub(crate) delta_input_handlers: Arc<Mutex<Vec<DeltaInputHandler>>>,

    /// Data directory for tests.
    pub data_directory: PathBuf,

    /// Shared in-memory SQLite database for tests.
    pub database: Arc<Mutex<signalk_sqlite::rusqlite::Connection>>,
}

impl MockPluginContext {
    pub fn new() -> Self {
        let db = signalk_sqlite::Database::open_in_memory().unwrap();
        MockPluginContext {
            emitted_deltas: Arc::new(Mutex::new(Vec::new())),
            registered_put_paths: Arc::new(Mutex::new(Vec::new())),
            status_messages: Arc::new(Mutex::new(Vec::new())),
            error_messages: Arc::new(Mutex::new(Vec::new())),
            stored_values: Arc::new(Mutex::new(HashMap::new())),
            saved_options: Arc::new(Mutex::new(None)),
            subscriptions: Arc::new(Mutex::new(Vec::new())),
            next_sub_id: Arc::new(Mutex::new(1)),
            delta_input_handlers: Arc::new(Mutex::new(Vec::new())),
            data_directory: PathBuf::from("/tmp/signalk-plugin-test"),
            database: Arc::new(Mutex::new(db.into_conn())),
        }
    }

    /// Pre-seed a path value for `get_self_path` to return.
    pub fn seed_value(&self, path: &str, value: serde_json::Value) {
        self.stored_values
            .lock()
            .unwrap()
            .insert(path.to_string(), value);
    }

    /// Deliver a delta to all active subscriptions (simulates store broadcast).
    ///
    /// Applies registered delta input handlers first — if any handler returns
    /// `None`, the delta is dropped and not delivered to subscriptions.
    pub fn deliver_delta(&self, delta: &Delta) {
        let handlers = self.delta_input_handlers.lock().unwrap();
        let mut current = delta.clone();
        for handler in handlers.iter() {
            match handler(current) {
                Some(d) => current = d,
                None => return, // dropped by handler
            }
        }
        drop(handlers);

        let subs = self.subscriptions.lock().unwrap();
        for (_, callback) in subs.iter() {
            callback(current.clone());
        }
    }

    /// Number of registered delta input handlers.
    pub fn delta_input_handler_count(&self) -> usize {
        self.delta_input_handlers.lock().unwrap().len()
    }
}

impl Default for MockPluginContext {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PluginContext for MockPluginContext {
    async fn get_self_path(&self, path: &str) -> Result<Option<serde_json::Value>, PluginError> {
        Ok(self.stored_values.lock().unwrap().get(path).cloned())
    }

    async fn get_path(&self, full_path: &str) -> Result<Option<serde_json::Value>, PluginError> {
        // Strip "vessels.self." prefix if present for lookup.
        let path = full_path.strip_prefix("vessels.self.").unwrap_or(full_path);
        Ok(self.stored_values.lock().unwrap().get(path).cloned())
    }

    async fn handle_message(&self, delta: Delta) -> Result<(), PluginError> {
        self.emitted_deltas.lock().unwrap().push(delta);
        Ok(())
    }

    async fn subscribe(
        &self,
        _spec: SubscriptionSpec,
        callback: DeltaCallback,
    ) -> Result<SubscriptionHandle, PluginError> {
        let mut id = self.next_sub_id.lock().unwrap();
        let handle = SubscriptionHandle::new(*id);
        *id += 1;
        self.subscriptions
            .lock()
            .unwrap()
            .push((handle.id(), callback));
        Ok(handle)
    }

    async fn unsubscribe(&self, handle: SubscriptionHandle) -> Result<(), PluginError> {
        self.subscriptions
            .lock()
            .unwrap()
            .retain(|(id, _)| *id != handle.id());
        Ok(())
    }

    async fn register_put_handler(
        &self,
        context: &str,
        path: &str,
        _handler: PutHandler,
    ) -> Result<(), PluginError> {
        self.registered_put_paths
            .lock()
            .unwrap()
            .push((context.to_string(), path.to_string()));
        Ok(())
    }

    async fn register_routes(&self, setup: RouterSetup) -> Result<(), PluginError> {
        // Call the setup to verify it doesn't panic, but discard routes.
        let mut collector = crate::context::RouteCollector::new();
        setup(&mut collector);
        Ok(())
    }

    async fn save_options(&self, opts: serde_json::Value) -> Result<(), PluginError> {
        *self.saved_options.lock().unwrap() = Some(opts);
        Ok(())
    }

    async fn read_options(&self) -> Result<serde_json::Value, PluginError> {
        Ok(self
            .saved_options
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new())))
    }

    fn data_dir(&self) -> PathBuf {
        self.data_directory.clone()
    }

    fn database(&self) -> Option<Arc<Mutex<signalk_sqlite::rusqlite::Connection>>> {
        Some(self.database.clone())
    }

    fn set_status(&self, msg: &str) {
        self.status_messages.lock().unwrap().push(msg.to_string());
    }

    fn set_error(&self, msg: &str) {
        self.error_messages.lock().unwrap().push(msg.to_string());
    }

    async fn register_delta_input_handler(
        &self,
        handler: DeltaInputHandler,
    ) -> Result<(), PluginError> {
        self.delta_input_handlers.lock().unwrap().push(handler);
        Ok(())
    }

    async fn register_autopilot_provider(
        &self,
        _provider: std::sync::Arc<dyn crate::autopilot::AutopilotProvider>,
    ) -> Result<(), PluginError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{SubscriptionSpec, delta_callback};
    use signalk_types::{Delta, PathValue, Source, Subscription, Update};

    #[tokio::test]
    async fn mock_get_self_path_returns_seeded_value() {
        let mock = MockPluginContext::new();
        mock.seed_value("navigation.speedOverGround", serde_json::json!(3.5));

        let result = mock
            .get_self_path("navigation.speedOverGround")
            .await
            .unwrap();
        assert_eq!(result, Some(serde_json::json!(3.5)));
    }

    #[tokio::test]
    async fn mock_get_self_path_returns_none_for_missing() {
        let mock = MockPluginContext::new();
        let result = mock
            .get_self_path("navigation.speedOverGround")
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn mock_handle_message_records_delta() {
        let mock = MockPluginContext::new();
        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(3.5),
            )],
        )]);

        mock.handle_message(delta.clone()).await.unwrap();

        let recorded = mock.emitted_deltas.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].context, delta.context);
    }

    #[tokio::test]
    async fn mock_subscribe_and_deliver() {
        let mock = MockPluginContext::new();
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        let handle = mock
            .subscribe(
                SubscriptionSpec::self_vessel(vec![Subscription::path("navigation.*")]),
                delta_callback(move |delta| {
                    received_clone.lock().unwrap().push(delta);
                }),
            )
            .await
            .unwrap();

        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(3.5),
            )],
        )]);

        mock.deliver_delta(&delta);
        assert_eq!(received.lock().unwrap().len(), 1);

        // Unsubscribe and verify no more deliveries.
        mock.unsubscribe(handle).await.unwrap();
        mock.deliver_delta(&delta);
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn mock_set_status_records() {
        let mock = MockPluginContext::new();
        mock.set_status("Running");
        mock.set_status("Connected to GPS");

        let messages = mock.status_messages.lock().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0], "Running");
        assert_eq!(messages[1], "Connected to GPS");
    }

    #[tokio::test]
    async fn mock_save_and_read_options() {
        let mock = MockPluginContext::new();
        let opts = serde_json::json!({"radius": 75.0});

        mock.save_options(opts.clone()).await.unwrap();
        let loaded = mock.read_options().await.unwrap();
        assert_eq!(loaded, opts);
    }

    #[tokio::test]
    async fn raise_notification_emits_correct_delta() {
        use signalk_types::{Notification, NotificationMethod, NotificationState};

        let mock = MockPluginContext::new();
        mock.raise_notification(
            "navigation.anchor",
            Notification {
                id: None,
                state: NotificationState::Alarm,
                method: vec![NotificationMethod::Visual, NotificationMethod::Sound],
                message: "Anchor dragging!".to_string(),
                status: None,
            },
            "anchor-alarm",
        )
        .await
        .unwrap();

        let deltas = mock.emitted_deltas.lock().unwrap();
        assert_eq!(deltas.len(), 1);

        let delta = &deltas[0];
        assert_eq!(delta.updates.len(), 1);
        assert_eq!(delta.updates[0].values.len(), 1);
        assert_eq!(
            delta.updates[0].values[0].path,
            "notifications.navigation.anchor"
        );

        let value = &delta.updates[0].values[0].value;
        assert_eq!(value["state"], "alarm");
        assert_eq!(value["message"], "Anchor dragging!");
        assert_eq!(value["method"], serde_json::json!(["visual", "sound"]));
    }

    #[tokio::test]
    async fn clear_notification_emits_normal_state() {
        let mock = MockPluginContext::new();
        mock.clear_notification("navigation.anchor", "anchor-alarm")
            .await
            .unwrap();

        let deltas = mock.emitted_deltas.lock().unwrap();
        assert_eq!(deltas.len(), 1);

        let value = &deltas[0].updates[0].values[0].value;
        assert_eq!(value["state"], "normal");
        assert!(value["method"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn mock_register_put_handler_records_path() {
        let mock = MockPluginContext::new();
        mock.register_put_handler(
            "vessels.self",
            "steering.autopilot.target.headingTrue",
            crate::context::put_handler(|_cmd| async move {
                Ok(crate::context::PutHandlerResult::Completed)
            }),
        )
        .await
        .unwrap();

        let paths = mock.registered_put_paths.lock().unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].0, "vessels.self");
        assert_eq!(paths[0].1, "steering.autopilot.target.headingTrue");
    }
}
