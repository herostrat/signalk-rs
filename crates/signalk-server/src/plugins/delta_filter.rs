/// Shared chain of delta input handlers (pre-store filters).
///
/// Plugins register handlers via `register_delta_input_handler`. Each handler
/// receives a delta and returns `Some(delta)` to pass through (possibly
/// modified), or `None` to drop it. Handlers are applied in registration order.
use signalk_plugin_api::DeltaInputHandler;
use signalk_types::Delta;
use std::sync::RwLock;

pub struct DeltaFilterChain {
    handlers: RwLock<Vec<(String, DeltaInputHandler)>>,
}

impl DeltaFilterChain {
    pub fn new() -> Self {
        DeltaFilterChain {
            handlers: RwLock::new(Vec::new()),
        }
    }

    /// Register a delta input handler for a plugin.
    pub fn register(&self, plugin_id: &str, handler: DeltaInputHandler) {
        self.handlers
            .write()
            .unwrap()
            .push((plugin_id.to_string(), handler));
    }

    /// Remove all handlers registered by a plugin.
    pub fn remove_plugin(&self, plugin_id: &str) {
        self.handlers
            .write()
            .unwrap()
            .retain(|(pid, _)| pid != plugin_id);
    }

    /// Apply all registered handlers to a delta.
    ///
    /// Returns `Some(delta)` if the delta passes all handlers (possibly modified),
    /// or `None` if any handler drops it.
    pub fn apply(&self, delta: Delta) -> Option<Delta> {
        let handlers = self.handlers.read().unwrap();
        let mut current = delta;
        for (_, handler) in handlers.iter() {
            match handler(current) {
                Some(d) => current = d,
                None => return None,
            }
        }
        Some(current)
    }
}

impl Default for DeltaFilterChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_types::{PathValue, Source, Update};

    fn make_delta(path: &str, value: f64) -> Delta {
        Delta::self_vessel(vec![Update::new(
            Source::plugin("test"),
            vec![PathValue::new(path, serde_json::json!(value))],
        )])
    }

    #[test]
    fn no_handlers_passes_through() {
        let chain = DeltaFilterChain::new();
        let delta = make_delta("navigation.speedOverGround", 3.5);
        let result = chain.apply(delta.clone());
        assert!(result.is_some());
    }

    #[test]
    fn handler_can_drop_delta() {
        let chain = DeltaFilterChain::new();
        chain.register("filter", Box::new(|_| None));

        let delta = make_delta("navigation.speedOverGround", 3.5);
        assert!(chain.apply(delta).is_none());
    }

    #[test]
    fn handler_can_pass_through() {
        let chain = DeltaFilterChain::new();
        chain.register("filter", Box::new(Some));

        let delta = make_delta("navigation.speedOverGround", 3.5);
        assert!(chain.apply(delta).is_some());
    }

    #[test]
    fn remove_plugin_clears_handlers() {
        let chain = DeltaFilterChain::new();
        chain.register("blocker", Box::new(|_| None));

        let delta = make_delta("navigation.speedOverGround", 3.5);
        assert!(chain.apply(delta).is_none());

        chain.remove_plugin("blocker");

        let delta = make_delta("navigation.speedOverGround", 3.5);
        assert!(chain.apply(delta).is_some());
    }
}
