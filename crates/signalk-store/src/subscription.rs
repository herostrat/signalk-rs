use signalk_types::{Delta, SubscriptionPolicy, Update};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A single active subscription from a WebSocket client.
#[derive(Debug, Clone)]
pub struct ActiveSubscription {
    pub context: String,
    pub path: String,
    pub period_ms: u64,
    pub policy: SubscriptionPolicy,
    pub min_period_ms: u64,
    pub last_sent: Option<Instant>,
    pub last_value: Option<serde_json::Value>,
}

impl ActiveSubscription {
    pub fn new(
        context: impl Into<String>,
        path: impl Into<String>,
        period_ms: u64,
        policy: SubscriptionPolicy,
        min_period_ms: u64,
    ) -> Self {
        ActiveSubscription {
            context: context.into(),
            path: path.into(),
            period_ms,
            policy,
            min_period_ms,
            last_sent: None,
            last_value: None,
        }
    }

    /// Whether this subscription should deliver a given delta path value right now.
    pub fn should_deliver(&self, path: &str, value: &serde_json::Value) -> bool {
        // Check path pattern match
        if !signalk_types::matches_pattern(&self.path, path) {
            return false;
        }

        let now = Instant::now();

        match self.policy {
            SubscriptionPolicy::Instant => {
                // Deliver immediately, but respect min_period
                match self.last_sent {
                    None => true,
                    Some(last) => now.duration_since(last) >= Duration::from_millis(self.min_period_ms),
                }
            }
            SubscriptionPolicy::Ideal => {
                // Deliver on change or when period elapsed
                if *value != self.last_value.clone().unwrap_or(serde_json::Value::Null) {
                    match self.last_sent {
                        None => true,
                        Some(last) => now.duration_since(last) >= Duration::from_millis(self.min_period_ms),
                    }
                } else {
                    match self.last_sent {
                        None => true,
                        Some(last) => now.duration_since(last) >= Duration::from_millis(self.period_ms),
                    }
                }
            }
            SubscriptionPolicy::Fixed => {
                // Deliver at fixed intervals only
                match self.last_sent {
                    None => true,
                    Some(last) => now.duration_since(last) >= Duration::from_millis(self.period_ms),
                }
            }
        }
    }

    pub fn mark_sent(&mut self, value: serde_json::Value) {
        self.last_sent = Some(Instant::now());
        self.last_value = Some(value);
    }
}

/// Filter a broadcast delta through a set of subscriptions, returning
/// a filtered delta containing only values matching any subscription.
///
/// Returns `None` if no values match.
pub fn filter_delta(
    delta: &Delta,
    subscriptions: &mut HashMap<String, ActiveSubscription>,
) -> Option<Delta> {
    let mut filtered_updates = Vec::new();

    for update in &delta.updates {
        let mut matched_values = Vec::new();

        for pv in &update.values {
            for sub in subscriptions.values_mut() {
                if sub.should_deliver(&pv.path, &pv.value) {
                    matched_values.push(pv.clone());
                    sub.mark_sent(pv.value.clone());
                    break; // One match per value is enough
                }
            }
        }

        if !matched_values.is_empty() {
            filtered_updates.push(Update::with_timestamp(
                update.source.clone(),
                update.timestamp,
                matched_values,
            ));
        }
    }

    if filtered_updates.is_empty() {
        None
    } else {
        Some(Delta {
            context: delta.context.clone(),
            updates: filtered_updates,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_types::{Delta, PathValue, Source, Update};

    fn nav_delta(sog: f64) -> Delta {
        Delta::self_vessel(vec![Update::new(
            Source::nmea0183("ttyUSB0", "GP"),
            vec![
                PathValue::new("navigation.speedOverGround", serde_json::json!(sog)),
                PathValue::new("navigation.courseOverGroundTrue", serde_json::json!(2.0)),
            ],
        )])
    }

    #[test]
    fn subscription_matches_exact_path() {
        let sub = ActiveSubscription::new(
            "vessels.self",
            "navigation.speedOverGround",
            1000,
            SubscriptionPolicy::Instant,
            0,
        );
        assert!(sub.should_deliver("navigation.speedOverGround", &serde_json::json!(3.5)));
        assert!(!sub.should_deliver("navigation.courseOverGroundTrue", &serde_json::json!(2.0)));
    }

    #[test]
    fn subscription_matches_wildcard() {
        let sub = ActiveSubscription::new(
            "vessels.self",
            "navigation.*",
            1000,
            SubscriptionPolicy::Instant,
            0,
        );
        assert!(sub.should_deliver("navigation.speedOverGround", &serde_json::json!(3.5)));
        assert!(sub.should_deliver("navigation.courseOverGroundTrue", &serde_json::json!(2.0)));
        assert!(!sub.should_deliver("propulsion.oilTemperature", &serde_json::json!(350.0)));
    }

    #[test]
    fn filter_delta_extracts_matching_values() {
        let mut subs = HashMap::new();
        subs.insert(
            "sub1".to_string(),
            ActiveSubscription::new(
                "vessels.self",
                "navigation.speedOverGround",
                1000,
                SubscriptionPolicy::Instant,
                0,
            ),
        );

        let delta = nav_delta(3.5);
        let filtered = filter_delta(&delta, &mut subs).unwrap();

        assert_eq!(filtered.updates[0].values.len(), 1);
        assert_eq!(filtered.updates[0].values[0].path, "navigation.speedOverGround");
    }

    #[test]
    fn filter_delta_returns_none_when_no_match() {
        let mut subs = HashMap::new();
        subs.insert(
            "sub1".to_string(),
            ActiveSubscription::new(
                "vessels.self",
                "propulsion.*",
                1000,
                SubscriptionPolicy::Instant,
                0,
            ),
        );

        let delta = nav_delta(3.5);
        assert!(filter_delta(&delta, &mut subs).is_none());
    }
}
