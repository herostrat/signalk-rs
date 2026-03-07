/// ConfigStore — unified configuration storage backed by SQLite.
///
/// All runtime-mutable config (vessel identity, source priorities/TTLs,
/// plugin configs) lives in the `config` table. On first start, values
/// are seeded from the TOML config; subsequent starts use the DB exclusively.
use serde::{Deserialize, Serialize};
use signalk_sqlite::rusqlite::Connection;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

/// A single plugin's persisted config + enabled flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfigEntry {
    pub enabled: bool,
    #[serde(default = "default_plugin_config")]
    pub config: serde_json::Value,
}

fn default_plugin_config() -> serde_json::Value {
    serde_json::json!({})
}

/// In-memory cache of all config values. Reads are lock-free (RwLock read).
struct ConfigCache {
    vessel_uuid: String,
    vessel_name: Option<String>,
    vessel_mmsi: Option<String>,
    source_priorities: HashMap<String, u16>,
    source_ttls: HashMap<String, u64>,
    plugins: HashMap<String, PluginConfigEntry>,
}

pub struct ConfigStore {
    db: Arc<std::sync::Mutex<Connection>>,
    cache: std::sync::RwLock<ConfigCache>,
}

impl ConfigStore {
    /// Create a new ConfigStore. If the DB config table is empty, seed from SeedConfig.
    pub fn new(
        db: Arc<std::sync::Mutex<Connection>>,
        config: &crate::config::SeedConfig,
    ) -> Self {
        let conn = db.lock().unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM config", [], |row| row.get(0))
            .unwrap_or(0);

        if count == 0 {
            info!("Config table empty — seeding from TOML");
            Self::seed_from_config(&conn, config);
        }

        let cache = Self::load_cache(&conn);
        drop(conn);

        ConfigStore {
            db,
            cache: std::sync::RwLock::new(cache),
        }
    }

    /// Seed initial config values from the SeedConfig.
    fn seed_from_config(conn: &Connection, config: &crate::config::SeedConfig) {
        // Vessel
        Self::upsert(conn, "server", "vessel_uuid", &json_str(&config.vessel.uuid));
        if let Some(ref name) = config.vessel.name {
            Self::upsert(conn, "server", "vessel_name", &json_str(name));
        }
        if let Some(ref mmsi) = config.vessel.mmsi {
            Self::upsert(conn, "server", "vessel_mmsi", &json_str(mmsi));
        }

        // Source priorities
        for (key, val) in &config.source_priorities {
            Self::upsert(
                conn,
                "source_priorities",
                key,
                &serde_json::to_string(val).unwrap(),
            );
        }

        // Source TTLs
        for (key, val) in &config.source_ttls {
            Self::upsert(
                conn,
                "source_ttls",
                key,
                &serde_json::to_string(val).unwrap(),
            );
        }

        // Plugins
        for pc in &config.plugins {
            let entry = PluginConfigEntry {
                enabled: pc.enabled,
                config: pc.config.clone(),
            };
            Self::upsert(
                conn,
                "plugins",
                &pc.id,
                &serde_json::to_string(&entry).unwrap(),
            );
        }
    }

    fn upsert(conn: &Connection, namespace: &str, key: &str, value: &str) {
        conn.execute(
            "INSERT INTO config (namespace, key, value, updated_at) VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(namespace, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            rusqlite::params![namespace, key, value],
        )
        .expect("config upsert failed");
    }

    /// Load the full cache from the DB.
    fn load_cache(conn: &Connection) -> ConfigCache {
        let mut cache = ConfigCache {
            vessel_uuid: String::new(),
            vessel_name: None,
            vessel_mmsi: None,
            source_priorities: HashMap::new(),
            source_ttls: HashMap::new(),
            plugins: HashMap::new(),
        };

        let mut stmt = conn
            .prepare("SELECT namespace, key, value FROM config")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap();

        for row in rows.flatten() {
            let (ns, key, value) = row;
            match ns.as_str() {
                "server" => match key.as_str() {
                    "vessel_uuid" => {
                        cache.vessel_uuid = from_json_str(&value);
                    }
                    "vessel_name" => {
                        cache.vessel_name = Some(from_json_str(&value));
                    }
                    "vessel_mmsi" => {
                        cache.vessel_mmsi = Some(from_json_str(&value));
                    }
                    _ => {}
                },
                "source_priorities" => {
                    if let Ok(v) = serde_json::from_str(&value) {
                        cache.source_priorities.insert(key, v);
                    }
                }
                "source_ttls" => {
                    if let Ok(v) = serde_json::from_str(&value) {
                        cache.source_ttls.insert(key, v);
                    }
                }
                "plugins" => {
                    if let Ok(entry) = serde_json::from_str(&value) {
                        cache.plugins.insert(key, entry);
                    }
                }
                _ => {}
            }
        }

        // Ensure we always have a UUID
        if cache.vessel_uuid.is_empty() {
            cache.vessel_uuid =
                format!("urn:mrn:signalk:uuid:{}", uuid::Uuid::new_v4());
        }

        cache
    }

    // ── Getters (sync, from cache) ────────────────────────────────────────

    pub fn vessel_uuid(&self) -> String {
        self.cache.read().unwrap().vessel_uuid.clone()
    }

    pub fn vessel_name(&self) -> Option<String> {
        self.cache.read().unwrap().vessel_name.clone()
    }

    pub fn vessel_mmsi(&self) -> Option<String> {
        self.cache.read().unwrap().vessel_mmsi.clone()
    }

    pub fn source_priorities(&self) -> HashMap<String, u16> {
        self.cache.read().unwrap().source_priorities.clone()
    }

    pub fn source_ttls(&self) -> HashMap<String, u64> {
        self.cache.read().unwrap().source_ttls.clone()
    }

    pub fn plugin_config(&self, id: &str) -> Option<PluginConfigEntry> {
        self.cache.read().unwrap().plugins.get(id).cloned()
    }

    pub fn all_plugin_configs(&self) -> HashMap<String, PluginConfigEntry> {
        self.cache.read().unwrap().plugins.clone()
    }

    // ── Setters (sync, DB write + cache update) ───────────────────────────

    pub fn set_vessel(
        &self,
        name: Option<String>,
        mmsi: Option<String>,
    ) {
        let conn = self.db.lock().unwrap();
        match &name {
            Some(n) => Self::upsert(&conn, "server", "vessel_name", &json_str(n)),
            None => {
                conn.execute(
                    "DELETE FROM config WHERE namespace = 'server' AND key = 'vessel_name'",
                    [],
                )
                .ok();
            }
        }
        match &mmsi {
            Some(m) => Self::upsert(&conn, "server", "vessel_mmsi", &json_str(m)),
            None => {
                conn.execute(
                    "DELETE FROM config WHERE namespace = 'server' AND key = 'vessel_mmsi'",
                    [],
                )
                .ok();
            }
        }
        drop(conn);

        let mut cache = self.cache.write().unwrap();
        cache.vessel_name = name;
        cache.vessel_mmsi = mmsi;
    }

    pub fn set_vessel_uuid(&self, uuid: &str) {
        let conn = self.db.lock().unwrap();
        Self::upsert(&conn, "server", "vessel_uuid", &json_str(uuid));
        drop(conn);

        self.cache.write().unwrap().vessel_uuid = uuid.to_string();
    }

    pub fn set_source_priorities(&self, priorities: HashMap<String, u16>) {
        let conn = self.db.lock().unwrap();
        // Clear old and insert new
        conn.execute(
            "DELETE FROM config WHERE namespace = 'source_priorities'",
            [],
        )
        .ok();
        for (key, val) in &priorities {
            Self::upsert(
                &conn,
                "source_priorities",
                key,
                &serde_json::to_string(val).unwrap(),
            );
        }
        drop(conn);

        self.cache.write().unwrap().source_priorities = priorities;
    }

    pub fn set_source_ttls(&self, ttls: HashMap<String, u64>) {
        let conn = self.db.lock().unwrap();
        conn.execute("DELETE FROM config WHERE namespace = 'source_ttls'", [])
            .ok();
        for (key, val) in &ttls {
            Self::upsert(
                &conn,
                "source_ttls",
                key,
                &serde_json::to_string(val).unwrap(),
            );
        }
        drop(conn);

        self.cache.write().unwrap().source_ttls = ttls;
    }

    pub fn set_plugin_config(&self, id: &str, config: &serde_json::Value) {
        let mut cache = self.cache.write().unwrap();
        let entry = cache
            .plugins
            .entry(id.to_string())
            .or_insert_with(|| PluginConfigEntry {
                enabled: true,
                config: serde_json::json!({}),
            });
        entry.config = config.clone();
        let serialized = serde_json::to_string(entry).unwrap();
        drop(cache);

        let conn = self.db.lock().unwrap();
        Self::upsert(&conn, "plugins", id, &serialized);
    }

    pub fn set_plugin_enabled(&self, id: &str, enabled: bool) {
        let mut cache = self.cache.write().unwrap();
        let entry = cache
            .plugins
            .entry(id.to_string())
            .or_insert_with(|| PluginConfigEntry {
                enabled,
                config: serde_json::json!({}),
            });
        entry.enabled = enabled;
        let serialized = serde_json::to_string(entry).unwrap();
        drop(cache);

        let conn = self.db.lock().unwrap();
        Self::upsert(&conn, "plugins", id, &serialized);
    }
}

use signalk_sqlite::rusqlite;

/// Wrap a string as a JSON string value for storage.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// Parse a JSON string value back to a plain string.
fn from_json_str(s: &str) -> String {
    serde_json::from_str(s).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SeedConfig;

    fn make_db() -> Arc<std::sync::Mutex<Connection>> {
        let db = signalk_sqlite::Database::open_in_memory().unwrap();
        Arc::new(std::sync::Mutex::new(db.into_conn()))
    }

    fn test_config() -> SeedConfig {
        let mut config = SeedConfig::default();
        config.vessel.uuid = "urn:mrn:signalk:uuid:test-1234".to_string();
        config.vessel.name = Some("TestVessel".to_string());
        config.vessel.mmsi = Some("211234567".to_string());
        config.source_priorities.insert("gps.GP".to_string(), 10);
        config.source_ttls.insert("nmea0183-tcp.GP".to_string(), 5);
        config.plugins.push(crate::config::PluginConfig {
            id: "test-plugin".to_string(),
            enabled: true,
            config: serde_json::json!({"addr": "0.0.0.0:10110"}),
        });
        config
    }

    #[test]
    fn test_first_start_seeds_from_toml() {
        let db = make_db();
        let config = test_config();
        let store = ConfigStore::new(db, &config);

        assert_eq!(store.vessel_uuid(), "urn:mrn:signalk:uuid:test-1234");
        assert_eq!(store.vessel_name(), Some("TestVessel".to_string()));
        assert_eq!(store.vessel_mmsi(), Some("211234567".to_string()));
        assert_eq!(store.source_priorities().get("gps.GP"), Some(&10));
        assert_eq!(store.source_ttls().get("nmea0183-tcp.GP"), Some(&5));

        let pc = store.plugin_config("test-plugin").unwrap();
        assert!(pc.enabled);
        assert_eq!(pc.config, serde_json::json!({"addr": "0.0.0.0:10110"}));
    }

    #[test]
    fn test_second_start_uses_db() {
        let db = make_db();
        let config = test_config();

        // First start: seeds from TOML
        let store = ConfigStore::new(db.clone(), &config);
        store.set_vessel(Some("Changed".to_string()), None);

        // Second start with different TOML: should use DB values
        let mut config2 = test_config();
        config2.vessel.name = Some("FromTOML".to_string());
        let store2 = ConfigStore::new(db, &config2);

        assert_eq!(store2.vessel_name(), Some("Changed".to_string()));
        assert_eq!(store2.vessel_mmsi(), None);
    }

    #[test]
    fn test_vessel_roundtrip() {
        let db = make_db();
        let store = ConfigStore::new(db, &test_config());

        store.set_vessel(Some("NewName".to_string()), Some("999999999".to_string()));
        assert_eq!(store.vessel_name(), Some("NewName".to_string()));
        assert_eq!(store.vessel_mmsi(), Some("999999999".to_string()));

        store.set_vessel_uuid("urn:mrn:signalk:uuid:new-uuid");
        assert_eq!(store.vessel_uuid(), "urn:mrn:signalk:uuid:new-uuid");
    }

    #[test]
    fn test_source_priorities_roundtrip() {
        let db = make_db();
        let store = ConfigStore::new(db, &test_config());

        let mut new_prios = HashMap::new();
        new_prios.insert("ais".to_string(), 50);
        store.set_source_priorities(new_prios);

        let prios = store.source_priorities();
        assert_eq!(prios.get("ais"), Some(&50));
        assert!(prios.get("gps.GP").is_none());
    }

    #[test]
    fn test_source_ttls_roundtrip() {
        let db = make_db();
        let store = ConfigStore::new(db, &test_config());

        let mut new_ttls = HashMap::new();
        new_ttls.insert("test.src".to_string(), 10);
        store.set_source_ttls(new_ttls);

        let ttls = store.source_ttls();
        assert_eq!(ttls.get("test.src"), Some(&10));
        assert!(ttls.get("nmea0183-tcp.GP").is_none());
    }

    #[test]
    fn test_plugin_config_roundtrip() {
        let db = make_db();
        let store = ConfigStore::new(db, &test_config());

        let new_config = serde_json::json!({"radius": 100});
        store.set_plugin_config("anchor-alarm", &new_config);

        let entry = store.plugin_config("anchor-alarm").unwrap();
        assert!(entry.enabled); // default
        assert_eq!(entry.config, new_config);
    }

    #[test]
    fn test_all_plugin_configs() {
        let db = make_db();
        let store = ConfigStore::new(db, &test_config());

        store.set_plugin_config("plugin-a", &serde_json::json!({}));
        store.set_plugin_config("plugin-b", &serde_json::json!({"x": 1}));

        let all = store.all_plugin_configs();
        // test-plugin from seed + plugin-a + plugin-b
        assert!(all.len() >= 3);
        assert!(all.contains_key("plugin-a"));
        assert!(all.contains_key("plugin-b"));
        assert!(all.contains_key("test-plugin"));
    }

    #[test]
    fn test_set_plugin_enabled() {
        let db = make_db();
        let store = ConfigStore::new(db, &test_config());

        // test-plugin is enabled from seed
        assert!(store.plugin_config("test-plugin").unwrap().enabled);

        store.set_plugin_enabled("test-plugin", false);
        assert!(!store.plugin_config("test-plugin").unwrap().enabled);
        // config preserved
        assert_eq!(
            store.plugin_config("test-plugin").unwrap().config,
            serde_json::json!({"addr": "0.0.0.0:10110"})
        );

        store.set_plugin_enabled("test-plugin", true);
        assert!(store.plugin_config("test-plugin").unwrap().enabled);
    }

    #[test]
    fn test_default_uuid_generated() {
        let db = make_db();
        let config = SeedConfig::default(); // vessel.uuid is empty by default

        let store = ConfigStore::new(db, &config);

        let uuid = store.vessel_uuid();
        assert!(
            uuid.starts_with("urn:mrn:signalk:uuid:"),
            "Expected generated UUID, got: {uuid}"
        );
    }
}
