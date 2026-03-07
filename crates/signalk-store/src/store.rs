use chrono::Utc;
use signalk_types::{
    Delta, FullModel, Metadata, SignalKValue, Source, SourceRef, SourceValue, Update, VesselData,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast};
use tracing::debug;

/// Capacity of the broadcast channel for delta fanout
const BROADCAST_CAPACITY: usize = 8192;

/// Default priority for sources without an explicit priority configuration.
/// Sources at the same priority level use last-write-wins semantics.
const DEFAULT_SOURCE_PRIORITY: u16 = 100;

/// The core in-memory SignalK data store.
///
/// Thread-safe via `Arc<RwLock<...>>`. Delta updates are stored in a flat
/// `HashMap<path, SignalKValue>` per vessel context and fanned out to all
/// WebSocket subscribers via a `tokio::broadcast` channel.
///
/// Multi-source: every value is stored per-source in `multi_source`, while
/// `vessel.values` always holds the highest-priority (lowest number) source.
/// When no priorities are configured, all sources default to 100 and the
/// behaviour is last-write-wins (backwards-compatible).
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

    /// Per-source values for the self vessel: path → (source_ref → SignalKValue).
    /// Every source's latest value is retained for source-selection queries.
    multi_source: HashMap<String, HashMap<String, SignalKValue>>,

    /// Source priority configuration: source_ref → priority (lower = higher).
    /// Sources not in this map default to DEFAULT_SOURCE_PRIORITY (100).
    source_priorities: HashMap<String, u16>,

    /// Source TTL configuration: source_ref → max age in seconds (lazy eviction).
    /// A source whose active value is older than its TTL is treated as "not present":
    /// any incoming source can overwrite it regardless of priority.
    /// Sources not in this map have no TTL (their values persist indefinitely).
    source_ttls: HashMap<String, u64>,

    /// Broadcast sender — all delta updates are sent here
    tx: broadcast::Sender<Delta>,

    /// Total number of deltas applied (monotonic counter for rate calculation)
    delta_count: Arc<AtomicU64>,

    /// Per-source delta counts: source label → monotonic counter.
    /// Used by admin dashboard for per-provider statistics.
    source_delta_counts: HashMap<String, u64>,
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
            multi_source: HashMap::new(),
            source_priorities: HashMap::new(),
            source_ttls: HashMap::new(),
            tx,
            delta_count: Arc::new(AtomicU64::new(0)),
            source_delta_counts: HashMap::new(),
        };
        (Arc::new(RwLock::new(store)), rx)
    }

    /// Set source priority configuration. Lower number = higher priority.
    /// Sources not in the map default to 100 (DEFAULT_SOURCE_PRIORITY).
    pub fn set_source_priorities(&mut self, priorities: HashMap<String, u16>) {
        self.source_priorities = priorities;
    }

    /// Set vessel identity fields (name, mmsi) on an existing vessel.
    pub fn set_vessel_identity(
        &mut self,
        vessel_uri: &str,
        name: Option<String>,
        mmsi: Option<String>,
    ) {
        if let Some(vessel) = self.vessels.get_mut(vessel_uri) {
            if let Some(n) = name {
                vessel.name = Some(n);
            }
            if let Some(m) = mmsi {
                vessel.mmsi = Some(m);
            }
        }
    }

    /// Set source TTL configuration. A source whose active value age exceeds its TTL
    /// is treated as "not present" during priority checks (lazy eviction).
    /// Sources not in the map have no TTL (values persist indefinitely).
    pub fn set_source_ttls(&mut self, ttls: HashMap<String, u64>) {
        self.source_ttls = ttls;
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

        let is_self = vessel_uri == self.self_uri;
        let vessel = self.vessels.entry(vessel_uri).or_default();

        let mut applied_updates: Vec<Update> = Vec::new();

        for update in &delta.updates {
            // Register source
            let source_ref = make_source_ref(&update.source);
            self.sources
                .entry(source_ref.0.clone())
                .or_insert_with(|| update.source.clone());

            // Track per-source delta count for admin dashboard
            let source_label = source_ref.0.split('.').next().unwrap_or(&source_ref.0);
            *self
                .source_delta_counts
                .entry(source_label.to_string())
                .or_insert(0) += 1;

            let mut applied_values: Vec<signalk_types::PathValue> = Vec::new();

            // Apply each value to the vessel's flat path map
            for pv in &update.values {
                let value = SignalKValue::new(
                    pv.value.clone(),
                    SourceRef::new(&source_ref.0),
                    update.timestamp,
                );

                // Always store in multi_source (retain all source values)
                if is_self {
                    self.multi_source
                        .entry(pv.path.clone())
                        .or_default()
                        .insert(source_ref.0.clone(), value.clone());
                }

                // Priority check with lazy TTL eviction:
                // A source whose active value has exceeded its configured TTL is treated
                // as "not present" — any incoming source can overwrite it.
                // Equal priority: last-write-wins. Lower number = higher priority.
                let new_pri = self
                    .source_priorities
                    .get(&source_ref.0)
                    .copied()
                    .unwrap_or(DEFAULT_SOURCE_PRIORITY);

                // Stale check: if the active value's source has a TTL and it expired, evict it.
                let evict_stale: Option<String> = vessel.values.get(&pv.path).and_then(|current| {
                    let ttl = self.source_ttls.get(&current.source.0).copied()?;
                    let age = Utc::now()
                        .signed_duration_since(current.timestamp)
                        .num_seconds();
                    if age > ttl as i64 {
                        Some(current.source.0.clone())
                    } else {
                        None
                    }
                });

                let should_update = evict_stale.is_some()
                    || match vessel.values.get(&pv.path) {
                        Some(current) => {
                            let cur_pri = self
                                .source_priorities
                                .get(&current.source.0)
                                .copied()
                                .unwrap_or(DEFAULT_SOURCE_PRIORITY);
                            new_pri <= cur_pri
                        }
                        None => true,
                    };

                if should_update {
                    // Evict stale source from multi_source so it no longer appears in `values`
                    if let Some(evicted) = evict_stale
                        && let Some(source_map) = self.multi_source.get_mut(&pv.path)
                    {
                        source_map.remove(&evicted);
                    }
                    vessel.values.insert(pv.path.clone(), value);
                    applied_values.push(pv.clone());
                }
            }

            if !applied_values.is_empty() {
                applied_updates.push(Update {
                    source: update.source.clone(),
                    timestamp: update.timestamp,
                    values: applied_values,
                });
            }
        }

        // Fan out only applied values to subscribers (ignore send errors — no receivers is fine)
        if !applied_updates.is_empty() {
            let applied_delta = Delta {
                context: delta.context,
                updates: applied_updates,
            };
            let _ = self.tx.send(applied_delta);
            self.delta_count.fetch_add(1, Ordering::Relaxed);
        }
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

    /// Get the value at a path from a specific source for the self vessel.
    pub fn get_self_path_by_source(&self, path: &str, source_ref: &str) -> Option<&SignalKValue> {
        self.multi_source.get(path)?.get(source_ref)
    }

    /// Get all source values for a path on the self vessel.
    /// Returns a map from source_ref → SignalKValue.
    pub fn get_self_path_sources(&self, path: &str) -> Option<&HashMap<String, SignalKValue>> {
        let sources = self.multi_source.get(path)?;
        if sources.is_empty() {
            None
        } else {
            Some(sources)
        }
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

    /// Get all notification paths and values for the self vessel.
    ///
    /// Returns all paths starting with `notifications.` and their values.
    /// To check for active alarms, inspect each value's `state` field.
    pub fn notifications(&self) -> Vec<(&str, &SignalKValue)> {
        let vessel = match self.vessels.get(&self.self_uri) {
            Some(v) => v,
            None => return vec![],
        };
        vessel
            .values
            .iter()
            .filter(|(path, _)| path.starts_with("notifications."))
            .map(|(path, val)| (path.as_str(), val))
            .collect()
    }

    /// Build the full SignalK model snapshot (for REST GET /signalk/v1/api/).
    ///
    /// Metadata is merged into SignalKValue leaves:
    /// 1. Explicit metadata (set via PUT /meta) takes highest priority
    /// 2. Spec defaults fill in for paths that have values but no explicit metadata
    ///
    /// Multi-source `values` are populated per spec: when 2+ sources provide a path,
    /// a `values` map is included on the leaf node.
    ///
    /// Sources are serialized as a hierarchical object (label → suffix → metadata).
    pub fn full_model(&self) -> FullModel {
        let mut model = FullModel::new(format!("vessels.{}", self.self_uri));

        for (uri, vessel) in &self.vessels {
            let mut vessel = vessel.clone();

            // Inject metadata + multi-source values for self vessel
            if uri == &self.self_uri {
                for (path, sv) in vessel.values.iter_mut() {
                    // Explicit metadata first, then spec defaults
                    if let Some(meta) = self.metadata.get(path) {
                        sv.meta = Some(meta.clone());
                    } else if let Some(meta) = signalk_types::meta::default_metadata(path) {
                        sv.meta = Some(meta);
                    }

                    // Populate per-source values when 2+ sources exist
                    if let Some(source_map) = self.multi_source.get(path)
                        && source_map.len() > 1
                    {
                        sv.values = Some(
                            source_map
                                .iter()
                                .map(|(src_ref, src_val)| {
                                    (
                                        src_ref.clone(),
                                        SourceValue {
                                            value: src_val.value.clone(),
                                            timestamp: src_val.timestamp,
                                        },
                                    )
                                })
                                .collect(),
                        );
                    }
                }
            }

            model.vessels.insert(uri.clone(), vessel);
        }

        model.sources = build_sources_hierarchy(&self.sources);

        model
    }

    /// Get the multi-source values for a self-vessel path.
    /// Returns None if the path has 0 or 1 sources.
    pub fn get_self_path_multi_values(&self, path: &str) -> Option<HashMap<String, SourceValue>> {
        let source_map = self.multi_source.get(path)?;
        if source_map.len() <= 1 {
            return None;
        }
        Some(
            source_map
                .iter()
                .map(|(src_ref, src_val)| {
                    (
                        src_ref.clone(),
                        SourceValue {
                            value: src_val.value.clone(),
                            timestamp: src_val.timestamp,
                        },
                    )
                })
                .collect(),
        )
    }

    /// Access the raw sources registry (for building the /sources endpoint).
    pub fn sources(&self) -> &HashMap<String, Source> {
        &self.sources
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

    /// Total number of deltas applied since server start.
    pub fn delta_count(&self) -> u64 {
        self.delta_count.load(Ordering::Relaxed)
    }

    /// Number of unique paths stored for the self vessel.
    pub fn self_path_count(&self) -> usize {
        self.vessels
            .get(&self.self_uri)
            .map(|v| v.values.len())
            .unwrap_or(0)
    }

    /// All dot-separated paths currently stored for the self vessel (sorted).
    pub fn self_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .vessels
            .get(&self.self_uri)
            .map(|v| v.values.keys().cloned().collect())
            .unwrap_or_default();
        paths.sort();
        paths
    }

    /// Source priority configuration: source_ref → priority (lower = higher).
    pub fn source_priorities(&self) -> &HashMap<String, u16> {
        &self.source_priorities
    }

    /// Per-source delta counts (source label → monotonic count).
    pub fn source_delta_counts(&self) -> &HashMap<String, u64> {
        &self.source_delta_counts
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

/// Build the hierarchical sources object per SignalK spec.
///
/// Sources with a suffix (NMEA0183/NMEA2000) are nested: `{label}.{suffix}`.
/// Sources without a suffix (plugins) are flat: `{label}`.
///
/// Example output:
/// ```json
/// {
///   "ttyUSB0": { "GP": { "label": "ttyUSB0", "type": "NMEA0183", "talker": "GP" } },
///   "sensor-data-simulator": { "label": "sensor-data-simulator", "type": "Plugin" }
/// }
/// ```
pub fn build_sources_hierarchy(sources: &HashMap<String, Source>) -> serde_json::Value {
    let mut root = serde_json::Map::new();

    for (ref_str, source) in sources {
        let source_json = serde_json::to_value(source).unwrap_or(serde_json::Value::Null);

        if let Some(dot_pos) = ref_str.find('.') {
            // Two-level: label.suffix
            let label = &ref_str[..dot_pos];
            let suffix = &ref_str[dot_pos + 1..];

            let label_entry = root
                .entry(label.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let serde_json::Value::Object(label_map) = label_entry {
                label_map.insert(suffix.to_string(), source_json);
            }
        } else {
            // Single-level: just the label (plugins, internal)
            root.insert(ref_str.clone(), source_json);
        }
    }

    serde_json::Value::Object(root)
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
        assert!(!model.sources.as_object().unwrap().is_empty());
    }

    // ── Multi-source / priority tests ────────────────────────────────────

    fn make_ais_delta(sog: f64) -> Delta {
        Delta::self_vessel(vec![Update::new(
            Source::plugin("ais"),
            vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(sog),
            )],
        )])
    }

    fn make_sim_delta(sog: f64) -> Delta {
        Delta::self_vessel(vec![Update::new(
            Source::plugin("simulator"),
            vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(sog),
            )],
        )])
    }

    #[test]
    fn multi_source_stores_all_sources() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        let sources = store
            .get_self_path_sources("navigation.speedOverGround")
            .unwrap();
        assert_eq!(sources.len(), 2);
        assert!(sources.contains_key("ttyUSB0.GP"));
        assert!(sources.contains_key("ais"));
    }

    #[test]
    fn no_priorities_last_write_wins() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        // Without priorities, AIS wrote last → AIS wins
        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(3.8));
        assert_eq!(active.source.0, "ais");
    }

    #[test]
    fn higher_priority_wins() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10); // GPS: highest
        priorities.insert("ais".to_string(), 50);
        priorities.insert("simulator".to_string(), 200); // Simulator: lowest
        store.set_source_priorities(priorities);

        // GPS writes first
        store.apply_delta(make_gps_delta(3.5));
        // AIS writes second — lower priority, should NOT overwrite
        store.apply_delta(make_ais_delta(3.8));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(3.5));
        assert_eq!(active.source.0, "ttyUSB0.GP");
    }

    #[test]
    fn lower_priority_does_not_overwrite() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("simulator".to_string(), 200);
        store.set_source_priorities(priorities);

        // GPS writes first
        store.apply_delta(make_gps_delta(3.5));
        // Simulator writes second — much lower priority, should NOT overwrite
        store.apply_delta(make_sim_delta(9.9));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(3.5), "GPS should still win");

        // But multi_source should have both
        let sources = store
            .get_self_path_sources("navigation.speedOverGround")
            .unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(
            sources.get("simulator").unwrap().value,
            serde_json::json!(9.9)
        );
    }

    #[test]
    fn same_priority_last_write_wins() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("ais".to_string(), 10); // Same priority as GPS
        store.set_source_priorities(priorities);

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        // Same priority → last write wins
        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(3.8));
        assert_eq!(active.source.0, "ais");
    }

    #[test]
    fn higher_priority_overwrites_lower() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("simulator".to_string(), 200);
        store.set_source_priorities(priorities);

        // Low-priority writes first
        store.apply_delta(make_sim_delta(9.9));
        // High-priority writes second — should overwrite
        store.apply_delta(make_gps_delta(3.5));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(3.5));
        assert_eq!(active.source.0, "ttyUSB0.GP");
    }

    #[test]
    fn get_self_path_by_source_returns_specific_source() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        let gps_val = store
            .get_self_path_by_source("navigation.speedOverGround", "ttyUSB0.GP")
            .unwrap();
        assert_eq!(gps_val.value, serde_json::json!(3.5));

        let ais_val = store
            .get_self_path_by_source("navigation.speedOverGround", "ais")
            .unwrap();
        assert_eq!(ais_val.value, serde_json::json!(3.8));

        assert!(
            store
                .get_self_path_by_source("navigation.speedOverGround", "unknown")
                .is_none()
        );
    }

    #[test]
    fn notifications_returns_notification_paths() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // Add regular data + notifications
        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(Delta::self_vessel(vec![Update::new(
            Source::plugin("anchor-alarm"),
            vec![PathValue::new(
                "notifications.navigation.anchor",
                serde_json::json!({
                    "state": "alarm",
                    "method": ["visual", "sound"],
                    "message": "Anchor dragging!"
                }),
            )],
        )]));

        let notifs = store.notifications();
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].0, "notifications.navigation.anchor");
        assert_eq!(notifs[0].1.value["state"], "alarm");

        // Regular navigation paths should NOT appear
        assert!(notifs.iter().all(|(p, _)| p.starts_with("notifications.")));
    }

    // ── Source hierarchy + values tests ───────────────────────────────

    #[test]
    fn build_sources_hierarchy_nmea0183() {
        let mut sources = HashMap::new();
        sources.insert("ttyUSB0.GP".to_string(), Source::nmea0183("ttyUSB0", "GP"));
        sources.insert("ttyUSB0.GN".to_string(), Source::nmea0183("ttyUSB0", "GN"));

        let hierarchy = build_sources_hierarchy(&sources);
        // Should be nested: ttyUSB0 → { GP: {...}, GN: {...} }
        assert!(hierarchy["ttyUSB0"]["GP"]["label"].is_string());
        assert_eq!(hierarchy["ttyUSB0"]["GP"]["label"], "ttyUSB0");
        assert_eq!(hierarchy["ttyUSB0"]["GP"]["type"], "NMEA0183");
        assert!(hierarchy["ttyUSB0"]["GN"]["label"].is_string());
    }

    #[test]
    fn build_sources_hierarchy_nmea2000() {
        let mut sources = HashMap::new();
        sources.insert(
            "n2k.129025".to_string(),
            Source::nmea2000("n2k", 42, 129025),
        );

        let hierarchy = build_sources_hierarchy(&sources);
        assert_eq!(hierarchy["n2k"]["129025"]["label"], "n2k");
        assert_eq!(hierarchy["n2k"]["129025"]["type"], "NMEA2000");
    }

    #[test]
    fn build_sources_hierarchy_plugin_flat() {
        let mut sources = HashMap::new();
        sources.insert("ais".to_string(), Source::plugin("ais"));

        let hierarchy = build_sources_hierarchy(&sources);
        // Plugin has no suffix → flat at top level
        assert_eq!(hierarchy["ais"]["label"], "ais");
        assert_eq!(hierarchy["ais"]["type"], "Plugin");
    }

    #[test]
    fn build_sources_hierarchy_mixed() {
        let mut sources = HashMap::new();
        sources.insert("ttyUSB0.GP".to_string(), Source::nmea0183("ttyUSB0", "GP"));
        sources.insert("simulator".to_string(), Source::plugin("simulator"));
        sources.insert(
            "n2k.128267".to_string(),
            Source::nmea2000("n2k", 42, 128267),
        );

        let hierarchy = build_sources_hierarchy(&sources);
        assert!(hierarchy["ttyUSB0"]["GP"].is_object());
        assert!(hierarchy["simulator"]["label"].is_string());
        assert!(hierarchy["n2k"]["128267"].is_object());
    }

    #[test]
    fn full_model_values_absent_for_single_source() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();
        store.apply_delta(make_gps_delta(3.5));

        let model = store.full_model();
        let vessel = model.vessels.get("urn:mrn:signalk:uuid:test").unwrap();
        let sog = vessel.values.get("navigation.speedOverGround").unwrap();
        assert!(
            sog.values.is_none(),
            "Single source should not have values field"
        );
    }

    #[test]
    fn full_model_values_present_for_multi_source() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        let model = store.full_model();
        let vessel = model.vessels.get("urn:mrn:signalk:uuid:test").unwrap();
        let sog = vessel.values.get("navigation.speedOverGround").unwrap();

        let values = sog
            .values
            .as_ref()
            .expect("Should have values for multi-source");
        assert_eq!(values.len(), 2);
        assert!(values.contains_key("ttyUSB0.GP"));
        assert!(values.contains_key("ais"));
        assert_eq!(values["ttyUSB0.GP"].value, serde_json::json!(3.5));
        assert_eq!(values["ais"].value, serde_json::json!(3.8));
    }

    #[test]
    fn full_model_sources_hierarchical() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        let model = store.full_model();
        // NMEA0183 source should be nested: ttyUSB0 → GP → {label, type, talker}
        assert_eq!(model.sources["ttyUSB0"]["GP"]["type"], "NMEA0183");
        // Plugin source should be flat: ais → {label, type}
        assert_eq!(model.sources["ais"]["type"], "Plugin");
    }

    #[test]
    fn get_self_path_multi_values_none_for_single_source() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();
        store.apply_delta(make_gps_delta(3.5));

        assert!(
            store
                .get_self_path_multi_values("navigation.speedOverGround")
                .is_none()
        );
    }

    #[test]
    fn get_self_path_multi_values_some_for_multi_source() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();
        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        let values = store
            .get_self_path_multi_values("navigation.speedOverGround")
            .expect("Should have multi-source values");
        assert_eq!(values.len(), 2);
    }

    // ── 3+ source conflict tests ─────────────────────────────────────

    fn make_nmea2000_delta(sog: f64) -> Delta {
        Delta::self_vessel(vec![Update::new(
            Source::nmea2000("n2k", 42, 129026),
            vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(sog),
            )],
        )])
    }

    #[test]
    fn three_source_highest_priority_wins() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10); // GPS: highest
        priorities.insert("ais".to_string(), 50); // AIS: medium
        priorities.insert("n2k.129026".to_string(), 200); // NMEA2000: lowest
        store.set_source_priorities(priorities);

        // All three write in order: GPS, AIS, NMEA2000
        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));
        store.apply_delta(make_nmea2000_delta(4.1));

        // GPS (priority 10) should be active
        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(3.5));
        assert_eq!(active.source.0, "ttyUSB0.GP");

        // All three in multi_source
        let sources = store
            .get_self_path_sources("navigation.speedOverGround")
            .unwrap();
        assert_eq!(sources.len(), 3);
        assert!(sources.contains_key("ttyUSB0.GP"));
        assert!(sources.contains_key("ais"));
        assert!(sources.contains_key("n2k.129026"));
    }

    #[test]
    fn three_source_middle_priority_does_not_overwrite_highest() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("ais".to_string(), 50);
        priorities.insert("n2k.129026".to_string(), 200);
        store.set_source_priorities(priorities);

        // Low writes first, then high, then middle
        store.apply_delta(make_nmea2000_delta(4.1));
        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8)); // AIS (50) should NOT overwrite GPS (10)

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(3.5));
        assert_eq!(active.source.0, "ttyUSB0.GP");
    }

    #[test]
    fn three_source_values_in_full_model() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));
        store.apply_delta(make_nmea2000_delta(4.1));

        let model = store.full_model();
        let vessel = model.vessels.get("urn:mrn:signalk:uuid:test").unwrap();
        let sog = vessel.values.get("navigation.speedOverGround").unwrap();

        let values = sog
            .values
            .as_ref()
            .expect("Should have values for 3-source");
        assert_eq!(values.len(), 3);
        assert_eq!(values["ttyUSB0.GP"].value, serde_json::json!(3.5));
        assert_eq!(values["ais"].value, serde_json::json!(3.8));
        assert_eq!(values["n2k.129026"].value, serde_json::json!(4.1));
    }

    #[test]
    fn three_source_highest_updates_active_value() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("ais".to_string(), 50);
        priorities.insert("n2k.129026".to_string(), 200);
        store.set_source_priorities(priorities);

        // All three write
        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));
        store.apply_delta(make_nmea2000_delta(4.1));

        // GPS updates with new value — should update active
        store.apply_delta(make_gps_delta(3.9));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(3.9));
        assert_eq!(active.source.0, "ttyUSB0.GP");

        // Multi-source GPS entry should also be updated
        let gps_val = store
            .get_self_path_by_source("navigation.speedOverGround", "ttyUSB0.GP")
            .unwrap();
        assert_eq!(gps_val.value, serde_json::json!(3.9));
    }

    // ── Default priority tests ───────────────────────────────────────

    #[test]
    fn default_priority_is_100_when_not_configured() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // Only configure GPS priority — AIS and simulator get default (100)
        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        store.set_source_priorities(priorities);

        // GPS (10) writes first
        store.apply_delta(make_gps_delta(3.5));
        // AIS (default 100) writes second — should NOT overwrite
        store.apply_delta(make_ais_delta(3.8));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(
            active.source.0, "ttyUSB0.GP",
            "GPS (10) should beat AIS (default 100)"
        );
    }

    #[test]
    fn two_unconfigured_sources_use_default_100_last_write_wins() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // No priorities configured — all sources get default 100
        store.apply_delta(make_ais_delta(3.8));
        store.apply_delta(make_sim_delta(9.9));

        // Both at default 100 → last write wins
        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(9.9));
        assert_eq!(active.source.0, "simulator");
    }

    #[test]
    fn configured_low_priority_loses_to_default() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // Only simulator configured to low priority (200)
        // AIS not configured → default 100
        let mut priorities = HashMap::new();
        priorities.insert("simulator".to_string(), 200);
        store.set_source_priorities(priorities);

        // AIS writes first (default 100)
        store.apply_delta(make_ais_delta(3.8));
        // Simulator (200) writes second — should NOT overwrite
        store.apply_delta(make_sim_delta(9.9));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(
            active.source.0, "ais",
            "AIS (default 100) should beat simulator (200)"
        );
    }

    // ── Dynamic priority change tests ────────────────────────────────

    #[test]
    fn dynamic_priority_change_allows_lower_source_to_win() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // Initial: GPS highest
        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("ais".to_string(), 50);
        store.set_source_priorities(priorities);

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        assert_eq!(
            store
                .get_self_path("navigation.speedOverGround")
                .unwrap()
                .source
                .0,
            "ttyUSB0.GP"
        );

        // Change priorities: AIS now higher than GPS
        let mut new_priorities = HashMap::new();
        new_priorities.insert("ttyUSB0.GP".to_string(), 50);
        new_priorities.insert("ais".to_string(), 10);
        store.set_source_priorities(new_priorities);

        // AIS writes again — should now overwrite GPS (AIS now priority 10)
        store.apply_delta(make_ais_delta(4.2));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(active.value, serde_json::json!(4.2));
        assert_eq!(active.source.0, "ais", "AIS should win after priority swap");
    }

    #[test]
    fn dynamic_priority_change_prevents_former_winner() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // Initial: GPS highest
        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("ais".to_string(), 50);
        store.set_source_priorities(priorities);

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        // Swap: AIS now highest
        let mut new_priorities = HashMap::new();
        new_priorities.insert("ttyUSB0.GP".to_string(), 50);
        new_priorities.insert("ais".to_string(), 10);
        store.set_source_priorities(new_priorities);

        // AIS takes over
        store.apply_delta(make_ais_delta(4.2));
        assert_eq!(
            store
                .get_self_path("navigation.speedOverGround")
                .unwrap()
                .source
                .0,
            "ais"
        );

        // GPS (now 50) writes — should NOT overwrite AIS (now 10)
        store.apply_delta(make_gps_delta(5.0));
        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(
            active.source.0, "ais",
            "GPS (50) should not overwrite AIS (10) after swap"
        );
        assert_eq!(active.value, serde_json::json!(4.2));
    }

    // ── Source eviction / stale source behavior ──────────────────────

    #[test]
    fn stale_high_priority_source_blocks_lower_sources_without_ttl() {
        // Without TTL configured: a high-priority source that stops sending keeps its value
        // active and lower-priority updates cannot overwrite it.
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("ais".to_string(), 50);
        store.set_source_priorities(priorities);

        // GPS writes once, then "goes offline" (stops sending)
        store.apply_delta(make_gps_delta(3.5));

        // AIS keeps sending — but cannot overwrite GPS (no TTL configured)
        store.apply_delta(make_ais_delta(3.8));
        store.apply_delta(make_ais_delta(4.0));
        store.apply_delta(make_ais_delta(4.2));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(
            active.value,
            serde_json::json!(3.5),
            "Without TTL, stale GPS value persists"
        );
        assert_eq!(active.source.0, "ttyUSB0.GP");

        // AIS values are still tracked in multi_source
        let ais_val = store
            .get_self_path_by_source("navigation.speedOverGround", "ais")
            .unwrap();
        assert_eq!(
            ais_val.value,
            serde_json::json!(4.2),
            "AIS latest value should be in multi_source"
        );
    }

    #[test]
    fn stale_source_allows_lower_priority_update() {
        use chrono::Duration;
        // With TTL configured: when GPS value is older than TTL, it is evicted and
        // lower-priority sources (like NTP/system-info) can take over.
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("ais".to_string(), 50);
        store.set_source_priorities(priorities);

        // GPS TTL: 5 seconds
        let mut ttls = HashMap::new();
        ttls.insert("ttyUSB0.GP".to_string(), 5u64);
        store.set_source_ttls(ttls);

        // GPS writes with a timestamp far in the past (simulates old/stale value)
        let old_ts = Utc::now() - Duration::seconds(30);
        let stale_gps_delta = Delta::self_vessel(vec![Update {
            source: Source::nmea0183("ttyUSB0", "GP"),
            timestamp: old_ts,
            values: vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(3.5),
            )],
        }]);
        store.apply_delta(stale_gps_delta);

        // Active value is GPS initially
        assert_eq!(
            store
                .get_self_path("navigation.speedOverGround")
                .unwrap()
                .source
                .0,
            "ttyUSB0.GP"
        );

        // AIS writes — GPS value is 30s old with 5s TTL → GPS is evicted → AIS wins
        store.apply_delta(make_ais_delta(4.2));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(
            active.value,
            serde_json::json!(4.2),
            "AIS should take over after GPS TTL expired"
        );
        assert_eq!(active.source.0, "ais");

        // Evicted GPS source is removed from multi_source
        assert!(
            store
                .get_self_path_by_source("navigation.speedOverGround", "ttyUSB0.GP")
                .is_none(),
            "Evicted GPS source should be removed from multi_source"
        );
    }

    #[test]
    fn fresh_high_priority_source_still_wins_over_ttl() {
        use chrono::Duration;
        // TTL only evicts when the value is actually old. A freshly-updated high-priority
        // source should still block lower-priority sources.
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("ais".to_string(), 50);
        store.set_source_priorities(priorities);

        let mut ttls = HashMap::new();
        ttls.insert("ttyUSB0.GP".to_string(), 5u64);
        store.set_source_ttls(ttls);

        // GPS writes with a fresh timestamp (within TTL)
        let fresh_ts = Utc::now() - Duration::seconds(2); // 2s old < 5s TTL
        let fresh_gps = Delta::self_vessel(vec![Update {
            source: Source::nmea0183("ttyUSB0", "GP"),
            timestamp: fresh_ts,
            values: vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(3.5),
            )],
        }]);
        store.apply_delta(fresh_gps);

        // AIS writes — GPS is fresh → AIS cannot overwrite
        store.apply_delta(make_ais_delta(4.2));

        let active = store.get_self_path("navigation.speedOverGround").unwrap();
        assert_eq!(
            active.value,
            serde_json::json!(3.5),
            "Fresh GPS should still win over lower-priority AIS"
        );
        assert_eq!(active.source.0, "ttyUSB0.GP");
    }

    // ── Values field timestamps + content ────────────────────────────

    #[test]
    fn values_field_preserves_different_timestamps() {
        use chrono::{DateTime, Utc};

        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // Create deltas with explicit different timestamps
        let ts1: DateTime<Utc> = "2024-06-01T10:00:00Z".parse().unwrap();
        let ts2: DateTime<Utc> = "2024-06-01T10:00:05Z".parse().unwrap();

        let delta1 = Delta::self_vessel(vec![Update {
            source: Source::nmea0183("ttyUSB0", "GP"),
            timestamp: ts1,
            values: vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(3.5),
            )],
        }]);
        let delta2 = Delta::self_vessel(vec![Update {
            source: Source::plugin("ais"),
            timestamp: ts2,
            values: vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(3.8),
            )],
        }]);

        store.apply_delta(delta1);
        store.apply_delta(delta2);

        let model = store.full_model();
        let vessel = model.vessels.get("urn:mrn:signalk:uuid:test").unwrap();
        let sog = vessel.values.get("navigation.speedOverGround").unwrap();

        let values = sog
            .values
            .as_ref()
            .expect("Should have multi-source values");
        assert_eq!(values["ttyUSB0.GP"].timestamp, ts1);
        assert_eq!(values["ais"].timestamp, ts2);
        assert_ne!(
            values["ttyUSB0.GP"].timestamp, values["ais"].timestamp,
            "Timestamps should differ between sources"
        );
    }

    #[test]
    fn values_field_appears_when_second_source_arrives() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // Single source — no values field
        store.apply_delta(make_gps_delta(3.5));
        assert!(
            store
                .get_self_path_multi_values("navigation.speedOverGround")
                .is_none(),
            "Single source should not have multi_values"
        );

        let model1 = store.full_model();
        let vessel1 = model1.vessels.get("urn:mrn:signalk:uuid:test").unwrap();
        let sog1 = vessel1.values.get("navigation.speedOverGround").unwrap();
        assert!(sog1.values.is_none(), "Single source → no values field");

        // Second source arrives — values field should now appear
        store.apply_delta(make_ais_delta(3.8));
        assert!(
            store
                .get_self_path_multi_values("navigation.speedOverGround")
                .is_some(),
            "Two sources should have multi_values"
        );

        let model2 = store.full_model();
        let vessel2 = model2.vessels.get("urn:mrn:signalk:uuid:test").unwrap();
        let sog2 = vessel2.values.get("navigation.speedOverGround").unwrap();
        let values = sog2.values.as_ref().expect("Two sources → values field");
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn values_field_content_matches_multi_source_values() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        let mut priorities = HashMap::new();
        priorities.insert("ttyUSB0.GP".to_string(), 10);
        priorities.insert("ais".to_string(), 50);
        priorities.insert("n2k.129026".to_string(), 200);
        store.set_source_priorities(priorities);

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));
        store.apply_delta(make_nmea2000_delta(4.1));

        let model = store.full_model();
        let vessel = model.vessels.get("urn:mrn:signalk:uuid:test").unwrap();
        let sog = vessel.values.get("navigation.speedOverGround").unwrap();

        // Active value should be GPS (highest priority)
        assert_eq!(sog.value, serde_json::json!(3.5));
        assert_eq!(sog.source.0, "ttyUSB0.GP");

        // values field should contain all three with their individual values
        let values = sog.values.as_ref().unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values["ttyUSB0.GP"].value, serde_json::json!(3.5));
        assert_eq!(values["ais"].value, serde_json::json!(3.8));
        assert_eq!(values["n2k.129026"].value, serde_json::json!(4.1));
    }

    #[test]
    fn full_model_serializes_values_correctly() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_ais_delta(3.8));

        let model = store.full_model();
        let json = serde_json::to_value(&model).unwrap();

        // Navigate the nested JSON to the values field
        let sog = &json["vessels"]["urn:mrn:signalk:uuid:test"]["navigation"]["speedOverGround"];
        assert!(
            sog["values"].is_object(),
            "values should be an object in JSON"
        );
        assert_eq!(sog["values"]["ttyUSB0.GP"]["value"], 3.5);
        assert_eq!(sog["values"]["ais"]["value"], 3.8);
        assert!(
            sog["values"]["ttyUSB0.GP"]["timestamp"].is_string(),
            "values entries should have timestamp"
        );
    }

    #[test]
    fn self_paths_returns_sorted_keys() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();
        store.apply_delta(make_gps_delta(3.5));

        let paths = store.self_paths();
        assert!(paths.contains(&"navigation.speedOverGround".to_string()));
        assert!(paths.contains(&"navigation.courseOverGroundTrue".to_string()));
        // Verify sorted order
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
    }

    #[test]
    fn source_delta_counts_tracks_per_source() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();

        // Apply 2 GPS deltas and 1 AIS delta
        store.apply_delta(make_gps_delta(3.5));
        store.apply_delta(make_gps_delta(3.6));
        store.apply_delta(Delta::self_vessel(vec![Update::new(
            Source::nmea0183("ais", "AI"),
            vec![PathValue::new(
                "navigation.speedOverGround",
                serde_json::json!(3.8),
            )],
        )]));

        let counts = store.source_delta_counts();
        assert_eq!(counts.get("ttyUSB0"), Some(&2));
        assert_eq!(counts.get("ais"), Some(&1));
    }

    #[test]
    fn source_priorities_exposed() {
        let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let mut store = store_arc.blocking_write();
        let mut prios = HashMap::new();
        prios.insert("gps.GP".to_string(), 10);
        prios.insert("ais".to_string(), 50);
        store.set_source_priorities(prios);

        let exposed = store.source_priorities();
        assert_eq!(exposed.get("gps.GP"), Some(&10));
        assert_eq!(exposed.get("ais"), Some(&50));
    }
}
