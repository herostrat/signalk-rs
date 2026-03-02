//! History ingestion: delta subscription and background maintenance tasks.

use super::config::{HistoryConfig, should_record};
use super::provider::HistoryProvider;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// Start the delta ingestion background task.
///
/// Subscribes to the store's broadcast channel, filters paths by the
/// configured include/exclude patterns, and writes numeric values in
/// batches at the configured sampling interval.
pub fn start_ingestion(
    mut rx: broadcast::Receiver<signalk_types::Delta>,
    provider: Arc<dyn HistoryProvider>,
    config: HistoryConfig,
) {
    let interval_ms = (config.sampling_interval_secs * 1000.0) as u64;

    tokio::spawn(async move {
        let mut batch: Vec<(String, String, f64, Option<String>)> = Vec::new();
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(interval_ms));

        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(delta) => {
                            let context = delta.context.as_deref().unwrap_or("vessels.self");
                            for update in &delta.updates {
                                let ts = update.timestamp.to_rfc3339();
                                for pv in &update.values {
                                    if !should_record(&pv.path, &config.include, &config.exclude) {
                                        continue;
                                    }
                                    // Only record numeric values
                                    if let Some(n) = pv.value.as_f64() {
                                        batch.push((
                                            context.to_string(),
                                            pv.path.clone(),
                                            n,
                                            Some(ts.clone()),
                                        ));
                                    }
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "History ingestion lagged — dropped deltas");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("History ingestion: broadcast channel closed, stopping");
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    if !batch.is_empty() {
                        let to_write = std::mem::take(&mut batch);
                        let provider = provider.clone();
                        // Use spawn_blocking for synchronous DB writes
                        tokio::task::spawn_blocking(move || {
                            let count = to_write.len();
                            if let Err(e) = provider.record_batch(&to_write) {
                                error!(error = %e, "History ingestion: batch write failed");
                            } else {
                                debug!(count, "History: recorded batch");
                            }
                        });
                    }
                }
            }
        }
    });
}

/// Start the periodic aggregation and retention maintenance task.
///
/// Runs every `aggregation_interval_secs` and:
/// 1. Aggregates raw data older than `retention_raw_days` into daily summaries
/// 2. Prunes raw data older than `retention_raw_days`
/// 3. Prunes daily data older than `retention_daily_days`
pub fn start_maintenance(provider: Arc<dyn HistoryProvider>, config: HistoryConfig) {
    let interval_secs = config.aggregation_interval_secs;
    let raw_days = config.retention_raw_days;
    let daily_days = config.retention_daily_days;
    let max_size = config.max_db_size_mb * 1024 * 1024;
    let vacuum_after = config.vacuum_after_prune;

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));

        // Skip the first immediate tick
        interval.tick().await;

        loop {
            interval.tick().await;

            let provider = provider.clone();
            tokio::task::spawn_blocking(move || {
                // 1. Time-based aggregation + pruning
                match provider.aggregate_and_prune(raw_days, daily_days) {
                    Ok(()) => debug!("History maintenance: aggregation complete"),
                    Err(e) => {
                        error!(error = %e, "History maintenance failed");
                        return;
                    }
                }

                // 2. Size-based check
                if max_size > 0 {
                    let over_limit = provider
                        .db_size_bytes()
                        .ok()
                        .is_some_and(|size| size > max_size);

                    if over_limit {
                        let size = provider.db_size_bytes().unwrap_or(0);
                        warn!(
                            size_mb = size / (1024 * 1024),
                            limit_mb = max_size / (1024 * 1024),
                            "History DB exceeds size limit — aggressive pruning"
                        );
                        let aggressive_raw = raw_days / 2;
                        let aggressive_daily = daily_days / 2;
                        if let Err(e) =
                            provider.aggregate_and_prune(aggressive_raw, aggressive_daily)
                        {
                            error!(error = %e, "Aggressive pruning failed");
                        }
                    }
                }

                // 3. Optional VACUUM after pruning
                if vacuum_after && provider.vacuum().is_err() {
                    warn!("VACUUM failed");
                }
            });
        }
    });
}
