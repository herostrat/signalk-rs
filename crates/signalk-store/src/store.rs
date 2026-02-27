use signalk_types::{Delta, FullModel, Metadata, SignalKValue, Source, SourceRef, VesselData};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tracing::debug;

/// Capacity of the broadcast channel for delta fanout
const BROADCAST_CAPACITY: usize = 1024;

/// The core in-memory SignalK data store.
///
/// Thread-safe via `Arc<RwLock<...>>`. Delta updates are stored in a flat
/// `HashMap<path, SignalKValue>` per vessel context and fanned out to all
/// WebSocket subscribers via a `tokio::broadcast` channel.
#[derive(Debug)]
pub struct SignalKStore {
    /// SignalK API version
    pub version: String,

    /// The local vessel's URI, e.g. "urn:mrn:signalk:uuid:..."
    pub self_uri: String,

    /// Vessel data indexed by vessel URI
    vessels: HashMap<String, VesselData>,

    /// Source registry: sourceRef string → Source object
    sources: HashMap<String, Source>,

    /// Per-path metadata (self vessel) — persists across delta updates.
    /// Explicit metadata set via PUT /meta takes precedence over defaults.
    metadata: HashMap<String, Metadata>,

    /// Broadcast sender — all delta updates are sent here
    tx: broadcast::Sender<Delta>,
}

impl SignalKStore {
    pub fn new(self_uri: impl Into<String>) -> (Arc<RwLock<Self>>, broadcast::Receiver<Delta>) {
        let (tx, rx) = broadcast::channel(BROADCAST_CAPACITY);
        let self_uri = self_uri.into();
        // Pre-populate the self vessel so GET /vessels/self returns 200 even with no data yet.
        let mut vessels = HashMap::new();
        vessels.insert(
            self_uri.clone(),
            VesselData {
                uuid: Some(self_uri.clone()),
                ..Default::default()
            },
        );
        let store = SignalKStore {
            version: signalk_types::SIGNALK_VERSION.to_string(),
            self_uri,
            vessels,
            sources: HashMap::new(),
            metadata: HashMap::new(),
            tx,
        };
        (Arc::new(RwLock::new(store)), rx)
    }

    /// Subscribe to the delta broadcast channel.
    /// Each subscriber gets its own receiver that sees all future deltas.
    pub fn subscribe(&self) -> broadcast::Receiver<Delta> {
        self.tx.subscribe()
    }

    /// Apply a delta update to the store and broadcast it to all subscribers.
    pub fn apply_delta(&mut self, delta: Delta) {
        let context = delta
            .context
            .clone()
            .unwrap_or_else(|| format!("vessels.{}", self.self_uri));

        // Resolve "vessels.self" to the actual URI
        let context = if context == "vessels.self" {
            format!("vessels.{}", self.self_uri)
        } else {
            context
        };

        // Extract vessel URI from context path "vessels.{uri}"
        let vessel_uri = if let Some(uri) = context.strip_prefix("vessels.") {
            uri.to_string()
        } else {
            // Non-vessel context (shore, aircraft, aton) — store not yet supported
            debug!(context = %context, "Ignoring non-vessel delta context");
            let _ = self.tx.send(delta);
            return;
        };

        let vessel = self.vessels.entry(vessel_uri).or_default();

        for update in &delta.updates {
            // Register source
            let source_ref = make_source_ref(&update.source);
            self.sources
                .entry(source_ref.0.clone())
                .or_insert_with(|| update.source.clone());

            // Apply each value to the vessel's flat path map
            for pv in &update.values {
                let value = SignalKValue::new(
                    pv.value.clone(),
                    SourceRef::new(&source_ref.0),
                    update.timestamp,
                );
                vessel.values.insert(pv.path.clone(), value);
            }
        }

        // Fan out to all WebSocket subscribers (ignore send errors — no receivers is fine)
        let _ = self.tx.send(delta);
    }

    /// Get the current value at a dot-path for the self vessel.
    pub fn get_self_path(&self, path: &str) -> Option<&SignalKValue> {
        let vessel = self.vessels.get(&self.self_uri)?;
        vessel.values.get(path)
    }

    /// Get the current value at a dot-path for a specific vessel URI.
    pub fn get_vessel_path(&self, vessel_uri: &str, path: &str) -> Option<&SignalKValue> {
        let vessel = self.vessels.get(vessel_uri)?;
        vessel.values.get(path)
    }

    /// Get all values for the self vessel matching a path pattern.
    pub fn get_self_matching(&self, pattern: &str) -> Vec<(&str, &SignalKValue)> {
        let vessel = match self.vessels.get(&self.self_uri) {
            Some(v) => v,
            None => return vec![],
        };
        vessel
            .values
            .iter()
            .filter(|(path, _)| signalk_types::matches_pattern(pattern, path))
            .map(|(path, val)| (path.as_str(), val))
            .collect()
    }

    /// Build the full SignalK model snapshot (for REST GET /signalk/v1/api/).
    ///
    /// Metadata is merged into SignalKValue leaves:
    /// 1. Explicit metadata (set via PUT /meta) takes highest priority
    /// 2. Spec defaults fill in for paths that have values but no explicit metadata
    pub fn full_model(&self) -> FullModel {
        let mut model = FullModel::new(&self.self_uri);

        for (uri, vessel) in &self.vessels {
            let mut vessel = vessel.clone();

            // Inject metadata for self vessel
            if uri == &self.self_uri {
                for (path, sv) in vessel.values.iter_mut() {
                    // Explicit metadata first, then spec defaults
                    if let Some(meta) = self.metadata.get(path) {
                        sv.meta = Some(meta.clone());
                    } else if let Some(meta) = signalk_types::meta::default_metadata(path) {
                        sv.meta = Some(meta);
                    }
                }
            }

            model.vessels.insert(uri.clone(), vessel);
        }

        for (ref_str, src) in &self.sources {
            model.sources.insert(
                ref_str.clone(),
                serde_json::to_value(src).unwrap_or(serde_json::Value::Null),
            );
        }

        model
    }

    /// Get a reference to vessel data for a specific URI.
    pub fn vessel(&self, uri: &str) -> Option<&VesselData> {
        self.vessels.get(uri)
    }

    /// Get mutable reference to vessel data for a specific URI.
    pub fn vessel_mut(&mut self, uri: &str) -> &mut VesselData {
        self.vessels.entry(uri.to_string()).or_default()
    }

    /// List all known vessel URIs.
    pub fn vessel_uris(&self) -> Vec<&str> {
        self.vessels.keys().map(String::as_str).collect()
    }

    /// Directly set a value in the self vessel (e.g. from internal PUT handler).
    pub fn set_self_path(&mut self, path: &str, value: serde_json::Value, source: Source) {
        use chrono::Utc;
        let source_ref = make_source_ref(&source);
        self.sources
            .entry(source_ref.0.clone())
            .or_insert_with(|| source.clone());

        let vessel = self.vessel_mut(&self.self_uri.clone());
        vessel.values.insert(
            path.to_string(),
            SignalKValue::new(value, SourceRef::new(&source_ref.0), Utc::now()),
        );
    }

    // ── Metadata ─────────────────────────────────────────────────────────────

    /// Set explicit metadata for a path (self vessel).
    pub fn set_metadata(&mut self, path: &str, meta: Metadata) {
        self.metadata.insert(path.to_string(), meta);
    }

    /// Get explicit metadata for a path (self vessel).
    pub fn get_metadata(&self, path: &str) -> Option<&Metadata> {
        self.metadata.get(path)
    }

    /// Get effective metadata: explicit first, then spec defaults.
    pub fn effective_metadata(&self, path: &str) -> Option<Metadata> {
        if let Some(meta) = self.metadata.get(path) {
            return Some(meta.clone());
        }
        signalk_types::meta::default_metadata(path)
    }
}

/// Build a source reference string from a Source object.
/// Convention: "{label}.{type_specific}" e.g. "ttyUSB0.GP"
fn make_source_ref(source: &Source) -> SourceRef {
    let suffix = match source.type_.as_str() {
        "NMEA0183" => source
            .extra
            .get("talker")
            .and_then(|v| v.as_str())
            .map(|t| format!(".{}", t))
            .unwrap_or_default(),
        "NMEA2000" => source
            .extra
            .get("pgn")
            .map(|v| format!(".{}", v))
            .unwrap_or_default(),
        _ => String::new(),
    };
    SourceRef::new(format!("{}{}", source.label, suffix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalk_types::{PathValue, Update};

    fn make_gps_delta(sog: f64) -> Delta {
        Delta::self_vessel(vec![Update::new(
            Source::nmea0183("ttyUSB0", "GP"),
            vec![
                PathValue::new("navigation.speedOverGround", serde_json::json!(sog)),
                PathValue::new("navigation.courseOverGroundTrue", serde_json::json!(2.971)),
            ],
        )])
    }

    #[test]
    fn apply_delta_stores_values() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));

        let sog = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(sog.value, serde_json::json!(3.5));
    }

    #[test]
    fn apply_delta_updates_value() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_gps_delta(4.2));

        let sog = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(sog.value, serde_json::json!(4.2));
    }

    #[test]
    fn apply_delta_resolves_vessels_self() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:abc123");
        let mut store = store_arc.blocking_write();

        // Delta with "vessels.self" context
        store.apply_delta(make_gps_delta(5.0));

        // Should be stored under the actual vessel URI
        let vessel_sog = store
            .get_vessel_path("urn:mrn:signalk:uuid:abc123", "navigation.speedOverGround")
            .unwrap();
        assert_eq!(vessel_sog.value, serde_json::json!(5.0));

        // And accessible via self shortcut
        let self_sog = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(self_sog.value, serde_json::json!(5.0));
    }

    #[test]
    fn source_ref_from_nmea0183() {
        let src = Source::nmea0183("ttyUSB0", "GP");
        let ref_ = make_source_ref(&src);
        assert_eq!(ref_.0, "ttyUSB0.GP");
    }

    #[test]
    fn broadcast_receiver_gets_delta() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (store_arc, mut rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
            {
                let mut store = store_arc.write().await;
                store.apply_delta(make_gps_delta(6.0));
            }
            let received = rx.recv().await.unwrap();
            assert_eq!(
                received.updates[0].values[0].path,
                "navigation.speedOverGround"
            );
        });
    }

    #[test]
    fn pattern_matching_get_self_matching() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();
        store.apply_delta(make_gps_delta(3.5));

        let nav = store.get_self_matching("navigation.*");
        assert_eq!(nav.len(), 2);
    }

    #[test]
    fn set_and_get_metadata() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let meta = Metadata {
            units: Some("m/s".to_string()),
            description: Some("Speed over ground".to_string()),
            ..Default::default()
        };
        store.set_metadata("navigation.speedOverGround", meta.clone());

        let got = store.get_metadata("navigation.speedOverGround").unwrap();
        assert_eq!(got.units, meta.units);
        assert!(store.get_metadata("navigation.unknown").is_none());
    }

    #[test]
    fn effective_metadata_prefers_explicit() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // Default exists for SOG
        let default = store
            .effective_metadata("navigation.speedOverGround")
            .unwrap();
        assert_eq!(default.units.as_deref(), Some("m/s"));

        // Override with explicit metadata (custom zones)
        let custom = Metadata {
            units: Some("kn".to_string()),
            ..Default::default()
        };
        store.set_metadata("navigation.speedOverGround", custom);

        let effective = store
            .effective_metadata("navigation.speedOverGround")
            .unwrap();
        assert_eq!(effective.units.as_deref(), Some("kn"));
    }

    #[test]
    fn full_model_includes_default_metadata() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));

        let model = store.full_model();
        let vessel = model.vessels.get("urn:mrn:signalk:uuid:test").unwrap();
        let sog = vessel.values.get("navigation.speedOverGround").unwrap();
        assert!(sog.meta.is_some(), "SOG should have default metadata");
        assert_eq!(sog.meta.as_ref().unwrap().units.as_deref(), Some("m/s"));
    }

    #[test]
    fn full_model_includes_explicit_metadata() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));
        store.set_metadata(
            "navigation.speedOverGround",
            Metadata {
                units: Some("kn".to_string()),
                description: Some("Custom SOG".to_string()),
                ..Default::default()
            },
        );

        let model = store.full_model();
        let vessel = model.vessels.get("urn:mrn:signalk:uuid:test").unwrap();
        let sog = vessel.values.get("navigation.speedOverGround").unwrap();
        // Explicit overrides default
        assert_eq!(sog.meta.as_ref().unwrap().units.as_deref(), Some("kn"));
        assert_eq!(
            sog.meta.as_ref().unwrap().description.as_deref(),
            Some("Custom SOG")
        );
    }

    #[test]
    fn full_model_includes_vessel() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();
        store.apply_delta(make_gps_delta(3.5));

        let model = store.full_model();
        assert!(model.vessels.contains_key("urn:mrn:signalk:uuid:test"));
        assert!(!model.sources.is_empty());
    }
}
