//! History subsystem: time-series storage, aggregation, and query.
//!
//! Provides the SignalK v2 History API backed by SQLite:
//! - **Ingestion**: subscribes to store broadcast, writes to `history_raw`
//! - **Aggregation**: periodically compacts raw → daily summaries
//! - **Query**: serves `/signalk/v2/api/history/{values,contexts,paths}`
//!
//! The default `SqliteHistoryProvider` can be replaced by an external plugin
//! via `HistoryManager::set_provider()` (e.g. for InfluxDB or Parquet).

mod config;
mod ingestion;
mod provider;
pub mod query;

pub use config::HistoryConfig;
pub use provider::{HistoryProvider, SqliteHistoryProvider};
pub use query::{ContextsRequest, PathsRequest, ValuesRequest, ValuesResponse};

use signalk_sqlite::rusqlite::Connection;
use signalk_store::store::SignalKStore;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tracing::info;

/// Central manager for the history subsystem.
///
/// Coordinates ingestion, aggregation, and queries using a shared database.
pub struct HistoryManager {
    provider: RwLock<Arc<dyn HistoryProvider>>,
    config: HistoryConfig,
}

impl HistoryManager {
    /// Create a new history manager using the shared database connection.
    pub fn new(config: HistoryConfig, conn: Arc<Mutex<Connection>>) -> Arc<Self> {
        if !config.enabled {
            info!("History subsystem disabled");
            let provider: Arc<dyn HistoryProvider> = Arc::new(DisabledProvider);
            return Arc::new(HistoryManager {
                provider: RwLock::new(provider),
                config,
            });
        }

        let provider = Arc::new(SqliteHistoryProvider::new(conn));
        Arc::new(HistoryManager {
            provider: RwLock::new(provider as Arc<dyn HistoryProvider>),
            config,
        })
    }

    /// Create a history manager with an in-memory database (for tests).
    pub fn new_in_memory(config: HistoryConfig) -> Arc<Self> {
        if !config.enabled {
            let provider: Arc<dyn HistoryProvider> = Arc::new(DisabledProvider);
            return Arc::new(HistoryManager {
                provider: RwLock::new(provider),
                config,
            });
        }

        let db = signalk_sqlite::Database::open_in_memory().expect("in-memory DB");
        let conn = Arc::new(Mutex::new(db.into_conn()));
        let provider = Arc::new(SqliteHistoryProvider::new(conn));
        Arc::new(HistoryManager {
            provider: RwLock::new(provider as Arc<dyn HistoryProvider>),
            config,
        })
    }

    /// Start the ingestion and aggregation background tasks.
    pub async fn start(self: &Arc<Self>, store: Arc<RwLock<SignalKStore>>) {
        if !self.config.enabled {
            return;
        }

        // Start delta ingestion
        let rx = store.read().await.subscribe();
        let provider = self.provider.read().await.clone();
        let config = self.config.clone();
        ingestion::start_ingestion(rx, provider, config);

        // Start aggregation/retention task
        let provider = self.provider.read().await.clone();
        let config = self.config.clone();
        ingestion::start_maintenance(provider, config);

        info!("History subsystem started");
    }

    /// Get the active history provider (for query handlers).
    pub async fn provider(&self) -> Arc<dyn HistoryProvider> {
        self.provider.read().await.clone()
    }

    /// Replace the active provider (for plugin-based overrides).
    #[allow(dead_code)]
    pub async fn set_provider(&self, provider: Arc<dyn HistoryProvider>) {
        *self.provider.write().await = provider;
    }

    /// Get the config.
    pub fn config(&self) -> &HistoryConfig {
        &self.config
    }
}

/// Placeholder provider when history is disabled.
struct DisabledProvider;

impl HistoryProvider for DisabledProvider {
    fn get_values(&self, _req: &ValuesRequest) -> Result<ValuesResponse, String> {
        Err("History is disabled".to_string())
    }

    fn get_contexts(&self, _req: &ContextsRequest) -> Result<Vec<String>, String> {
        Err("History is disabled".to_string())
    }

    fn get_paths(&self, _req: &PathsRequest) -> Result<Vec<String>, String> {
        Err("History is disabled".to_string())
    }

    fn record_batch(&self, _batch: &[(String, String, f64, Option<String>)]) -> Result<(), String> {
        Ok(()) // silently discard
    }

    fn aggregate_and_prune(
        &self,
        _retention_raw_days: u32,
        _retention_daily_days: u32,
    ) -> Result<(), String> {
        Ok(())
    }

    fn db_size_bytes(&self) -> Result<u64, String> {
        Ok(0)
    }

    fn vacuum(&self) -> Result<(), String> {
        Ok(())
    }
}
