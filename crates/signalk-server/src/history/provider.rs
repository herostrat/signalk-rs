//! History provider trait and SQLite implementation.

use super::query::{
    AggregateMethod, ContextsRequest, PathsRequest, TimeRange, ValueMeta, ValuesRequest,
    ValuesResponse,
};
use signalk_sqlite::rusqlite::Connection;
use std::sync::{Arc, Mutex};

/// Trait for history data providers.
///
/// The default implementation uses SQLite. Plugins can register alternative
/// providers (e.g. InfluxDB, Parquet) via `HistoryManager::set_provider()`.
pub trait HistoryProvider: Send + Sync {
    /// Query time-series values.
    fn get_values(&self, req: &ValuesRequest) -> Result<ValuesResponse, String>;

    /// List available contexts (vessels) with history data.
    fn get_contexts(&self, req: &ContextsRequest) -> Result<Vec<String>, String>;

    /// List available paths with history data.
    fn get_paths(&self, req: &PathsRequest) -> Result<Vec<String>, String>;

    /// Record a batch of values: `(context, path, value, timestamp)`.
    fn record_batch(&self, batch: &[(String, String, f64, Option<String>)]) -> Result<(), String>;

    /// Run aggregation (raw → daily) and retention pruning.
    fn aggregate_and_prune(
        &self,
        retention_raw_days: u32,
        retention_daily_days: u32,
    ) -> Result<(), String>;

    /// Return the current database size in bytes.
    fn db_size_bytes(&self) -> Result<u64, String>;

    /// Run VACUUM to reclaim disk space after deletions.
    fn vacuum(&self) -> Result<(), String>;
}

/// SQLite-backed history provider.
pub struct SqliteHistoryProvider {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteHistoryProvider {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        SqliteHistoryProvider { conn }
    }
}

impl HistoryProvider for SqliteHistoryProvider {
    fn record_batch(&self, batch: &[(String, String, f64, Option<String>)]) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("lock: {e}"))?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("tx: {e}"))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO history_raw (timestamp, context, path, value) \
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| format!("prepare: {e}"))?;

            for (context, path, value, timestamp) in batch {
                let ts = timestamp.as_deref().unwrap_or("");
                // Use current UTC if no timestamp provided
                let ts_str = if ts.is_empty() {
                    chrono::Utc::now().to_rfc3339()
                } else {
                    ts.to_string()
                };
                stmt.execute(signalk_sqlite::rusqlite::params![
                    ts_str, context, path, value
                ])
                .map_err(|e| format!("insert: {e}"))?;
            }
        }

        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(())
    }

    fn aggregate_and_prune(
        &self,
        retention_raw_days: u32,
        retention_daily_days: u32,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("lock: {e}"))?;

        // Aggregate old raw data into daily summaries
        conn.execute_batch(&format!(
                "INSERT OR REPLACE INTO history_daily (date, context, path, avg_value, min_value, max_value, count)
                 SELECT
                     substr(timestamp, 1, 10) AS date,
                     context, path,
                     AVG(value), MIN(value), MAX(value), COUNT(*)
                 FROM history_raw
                 WHERE timestamp < datetime('now', '-{retention_raw_days} days')
                   AND value IS NOT NULL
                 GROUP BY date, context, path;

                 DELETE FROM history_raw
                 WHERE timestamp < datetime('now', '-{retention_raw_days} days');

                 DELETE FROM history_daily
                 WHERE date < date('now', '-{retention_daily_days} days');"
            ))
            .map_err(|e| format!("aggregate: {e}"))?;

        Ok(())
    }

    fn db_size_bytes(&self) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| format!("lock: {e}"))?;
        let size: i64 = conn
            .query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("db_size: {e}"))?;
        Ok(size as u64)
    }

    fn vacuum(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("lock: {e}"))?;
        conn.execute_batch("VACUUM")
            .map_err(|e| format!("vacuum: {e}"))
    }

    fn get_values(&self, req: &ValuesRequest) -> Result<ValuesResponse, String> {
        let conn = self.conn.lock().map_err(|e| format!("lock: {e}"))?;
        let (from_ts, to_ts) = resolve_time_range(&req.from, &req.to, &req.duration)?;

        let explicit_resolution = req
            .resolution
            .as_deref()
            .and_then(super::query::parse_resolution_secs);

        // Auto-determine resolution if not specified (spec requirement)
        let resolution_secs = explicit_resolution.or_else(|| {
            let from_dt = chrono::DateTime::parse_from_rfc3339(&from_ts).ok()?;
            let to_dt = chrono::DateTime::parse_from_rfc3339(&to_ts).ok()?;
            let range_secs = (to_dt - from_dt).num_seconds() as f64;
            super::query::auto_resolution_secs(range_secs)
        });

        let mut all_data: Vec<Vec<serde_json::Value>> = Vec::new();
        let mut values_meta: Vec<ValueMeta> = Vec::new();

        for spec in &req.path_specs {
            let rows = query_path_values(
                &conn,
                &req.context,
                &spec.path,
                &from_ts,
                &to_ts,
                resolution_secs,
                spec.method,
            )?;

            values_meta.push(ValueMeta {
                path: spec.path.clone(),
                method: spec.method.as_str().to_string(),
            });

            // Merge rows into all_data
            if values_meta.len() == 1 {
                // First path — create rows
                all_data = rows
                    .into_iter()
                    .map(|(ts, val)| vec![serde_json::Value::String(ts), val])
                    .collect();
            } else {
                // Additional paths — merge by timestamp (simplified: append column)
                let col_idx = values_meta.len();
                // Build lookup from existing timestamps
                let mut ts_map: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for (i, row) in all_data.iter().enumerate() {
                    if let Some(ts) = row.first().and_then(|v| v.as_str()) {
                        ts_map.insert(ts.to_string(), i);
                    }
                }
                for (ts, val) in rows {
                    if let Some(&idx) = ts_map.get(&ts) {
                        // Pad if needed
                        while all_data[idx].len() < col_idx {
                            all_data[idx].push(serde_json::Value::Null);
                        }
                        all_data[idx].push(val);
                    } else {
                        // New timestamp — create row with nulls for prior columns
                        let mut row = vec![serde_json::Value::String(ts.clone())];
                        for _ in 1..col_idx {
                            row.push(serde_json::Value::Null);
                        }
                        row.push(val);
                        ts_map.insert(ts, all_data.len());
                        all_data.push(row);
                    }
                }
            }
        }

        // Sort by timestamp
        all_data.sort_by(|a, b| {
            let ta = a.first().and_then(|v| v.as_str()).unwrap_or("");
            let tb = b.first().and_then(|v| v.as_str()).unwrap_or("");
            ta.cmp(tb)
        });

        Ok(ValuesResponse {
            context: req.context.clone(),
            range: TimeRange {
                from: from_ts,
                to: to_ts,
            },
            values: values_meta,
            data: all_data,
        })
    }

    fn get_contexts(&self, req: &ContextsRequest) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().map_err(|e| format!("lock: {e}"))?;
        let (from_ts, to_ts) = resolve_time_range(&req.from, &req.to, &req.duration)?;

        let mut contexts: Vec<String> = Vec::new();

        // From raw
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT context FROM history_raw \
                 WHERE timestamp >= ?1 AND timestamp <= ?2",
            )
            .map_err(|e| format!("prepare: {e}"))?;
        let rows = stmt
            .query_map(signalk_sqlite::rusqlite::params![from_ts, to_ts], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| format!("query: {e}"))?;
        for row in rows.flatten() {
            if !contexts.contains(&row) {
                contexts.push(row);
            }
        }

        // From daily
        let from_date = &from_ts[..10.min(from_ts.len())];
        let to_date = &to_ts[..10.min(to_ts.len())];
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT context FROM history_daily \
                 WHERE date >= ?1 AND date <= ?2",
            )
            .map_err(|e| format!("prepare: {e}"))?;
        let rows = stmt
            .query_map(
                signalk_sqlite::rusqlite::params![from_date, to_date],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| format!("query: {e}"))?;
        for row in rows.flatten() {
            if !contexts.contains(&row) {
                contexts.push(row);
            }
        }

        contexts.sort();
        Ok(contexts)
    }

    fn get_paths(&self, req: &PathsRequest) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().map_err(|e| format!("lock: {e}"))?;
        let (from_ts, to_ts) = resolve_time_range(&req.from, &req.to, &req.duration)?;

        let mut paths: Vec<String> = Vec::new();

        // From raw
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT path FROM history_raw \
                 WHERE context = ?1 AND timestamp >= ?2 AND timestamp <= ?3",
            )
            .map_err(|e| format!("prepare: {e}"))?;
        let rows = stmt
            .query_map(
                signalk_sqlite::rusqlite::params![req.context, from_ts, to_ts],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| format!("query: {e}"))?;
        for row in rows.flatten() {
            if !paths.contains(&row) {
                paths.push(row);
            }
        }

        // From daily
        let from_date = &from_ts[..10.min(from_ts.len())];
        let to_date = &to_ts[..10.min(to_ts.len())];
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT path FROM history_daily \
                 WHERE context = ?1 AND date >= ?2 AND date <= ?3",
            )
            .map_err(|e| format!("prepare: {e}"))?;
        let rows = stmt
            .query_map(
                signalk_sqlite::rusqlite::params![req.context, from_date, to_date],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| format!("query: {e}"))?;
        for row in rows.flatten() {
            if !paths.contains(&row) {
                paths.push(row);
            }
        }

        paths.sort();
        Ok(paths)
    }
}

/// Resolve from/to/duration into concrete (from, to) ISO 8601 timestamps.
fn resolve_time_range(
    from: &Option<String>,
    to: &Option<String>,
    duration: &Option<String>,
) -> Result<(String, String), String> {
    let now = chrono::Utc::now();
    let now_str = now.to_rfc3339();

    match (from, to, duration) {
        // duration only → (now - duration, now)
        (None, None, Some(dur)) => {
            let secs = super::query::parse_duration_secs(dur)
                .ok_or_else(|| format!("Invalid duration: {dur}"))?;
            let from = (now - chrono::Duration::seconds(secs as i64)).to_rfc3339();
            Ok((from, now_str))
        }
        // from + duration → (from, from + duration)
        (Some(f), None, Some(dur)) => {
            let secs = super::query::parse_duration_secs(dur)
                .ok_or_else(|| format!("Invalid duration: {dur}"))?;
            let from_dt = chrono::DateTime::parse_from_rfc3339(f)
                .map_err(|e| format!("Invalid from: {e}"))?;
            let to_dt = from_dt + chrono::Duration::seconds(secs as i64);
            Ok((f.clone(), to_dt.to_rfc3339()))
        }
        // to + duration → (to - duration, to)
        (None, Some(t), Some(dur)) => {
            let secs = super::query::parse_duration_secs(dur)
                .ok_or_else(|| format!("Invalid duration: {dur}"))?;
            let to_dt =
                chrono::DateTime::parse_from_rfc3339(t).map_err(|e| format!("Invalid to: {e}"))?;
            let from_dt = to_dt - chrono::Duration::seconds(secs as i64);
            Ok((from_dt.to_rfc3339(), t.clone()))
        }
        // from only → (from, now)
        (Some(f), None, None) => Ok((f.clone(), now_str)),
        // from + to → (from, to)
        (Some(f), Some(t), _) => Ok((f.clone(), t.clone())),
        // No time params → last hour
        (None, None, None) => {
            let from = (now - chrono::Duration::hours(1)).to_rfc3339();
            Ok((from, now_str))
        }
        // to only without duration — not standard, use last hour ending at to
        (None, Some(t), None) => {
            let to_dt =
                chrono::DateTime::parse_from_rfc3339(t).map_err(|e| format!("Invalid to: {e}"))?;
            let from_dt = to_dt - chrono::Duration::hours(1);
            Ok((from_dt.to_rfc3339(), t.clone()))
        }
    }
}

/// Query values for a single path from the appropriate tier.
fn query_path_values(
    conn: &signalk_sqlite::rusqlite::Connection,
    context: &str,
    path: &str,
    from: &str,
    to: &str,
    resolution_secs: Option<f64>,
    method: AggregateMethod,
) -> Result<Vec<(String, serde_json::Value)>, String> {
    // Determine which tier to query based on whether raw data exists for this range
    let raw_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM history_raw \
             WHERE context = ?1 AND path = ?2 AND timestamp >= ?3 AND timestamp <= ?4",
            signalk_sqlite::rusqlite::params![context, path, from, to],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if raw_count > 0 {
        query_raw_values(conn, context, path, from, to, resolution_secs, method)
    } else {
        query_daily_values(conn, context, path, from, to, method)
    }
}

fn query_raw_values(
    conn: &signalk_sqlite::rusqlite::Connection,
    context: &str,
    path: &str,
    from: &str,
    to: &str,
    resolution_secs: Option<f64>,
    method: AggregateMethod,
) -> Result<Vec<(String, serde_json::Value)>, String> {
    let bucket_format = match resolution_secs {
        Some(s) if s >= 86400.0 => Some("%Y-%m-%dT00:00:00Z"),
        Some(s) if s >= 3600.0 => Some("%Y-%m-%dT%H:00:00Z"),
        Some(s) if s >= 60.0 => Some("%Y-%m-%dT%H:%M:00Z"),
        _ => None, // full resolution
    };

    let results = if let Some(fmt) = bucket_format {
        // mid/middle_index need all values per bucket for Rust-side computation
        if matches!(method, AggregateMethod::Mid | AggregateMethod::MiddleIndex) {
            query_raw_bucketed_rust(conn, context, path, from, to, fmt, method)?
        } else {
            let agg_fn = match method {
                AggregateMethod::Average => "AVG(value)",
                AggregateMethod::Min => "MIN(value)",
                AggregateMethod::Max => "MAX(value)",
                AggregateMethod::First => "MIN(value)", // approximate
                AggregateMethod::Last => "MAX(value)",  // approximate
                AggregateMethod::Count => "COUNT(value)",
                _ => "AVG(value)",
            };
            let sql = format!(
                "SELECT strftime('{fmt}', timestamp) AS ts, {agg_fn} AS val \
                 FROM history_raw \
                 WHERE context = ?1 AND path = ?2 AND timestamp >= ?3 AND timestamp <= ?4 \
                   AND value IS NOT NULL \
                 GROUP BY ts ORDER BY ts"
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
            let rows = stmt
                .query_map(
                    signalk_sqlite::rusqlite::params![context, path, from, to],
                    |row| {
                        let ts: String = row.get(0)?;
                        let val: f64 = row.get(1)?;
                        Ok((ts, val))
                    },
                )
                .map_err(|e| format!("query: {e}"))?;
            rows.filter_map(|r| r.ok())
                .map(|(ts, val)| (ts, serde_json::json!(val)))
                .collect()
        }
    } else {
        // Full resolution — no grouping
        let mut stmt = conn
            .prepare(
                "SELECT timestamp, value FROM history_raw \
                 WHERE context = ?1 AND path = ?2 AND timestamp >= ?3 AND timestamp <= ?4 \
                   AND value IS NOT NULL \
                 ORDER BY timestamp",
            )
            .map_err(|e| format!("prepare: {e}"))?;
        let rows = stmt
            .query_map(
                signalk_sqlite::rusqlite::params![context, path, from, to],
                |row| {
                    let ts: String = row.get(0)?;
                    let val: f64 = row.get(1)?;
                    Ok((ts, val))
                },
            )
            .map_err(|e| format!("query: {e}"))?;
        rows.filter_map(|r| r.ok())
            .map(|(ts, val)| (ts, serde_json::json!(val)))
            .collect()
    };

    Ok(results)
}

/// Bucketed query with Rust-side aggregation for mid/middle_index.
///
/// Fetches all values per bucket, then computes the result in Rust
/// since SQLite has no native MEDIAN function.
fn query_raw_bucketed_rust(
    conn: &signalk_sqlite::rusqlite::Connection,
    context: &str,
    path: &str,
    from: &str,
    to: &str,
    bucket_format: &str,
    method: AggregateMethod,
) -> Result<Vec<(String, serde_json::Value)>, String> {
    let sql = format!(
        "SELECT strftime('{bucket_format}', timestamp) AS ts, value \
         FROM history_raw \
         WHERE context = ?1 AND path = ?2 AND timestamp >= ?3 AND timestamp <= ?4 \
           AND value IS NOT NULL \
         ORDER BY ts, timestamp"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map(
            signalk_sqlite::rusqlite::params![context, path, from, to],
            |row| {
                let ts: String = row.get(0)?;
                let val: f64 = row.get(1)?;
                Ok((ts, val))
            },
        )
        .map_err(|e| format!("query: {e}"))?;

    // Group values by bucket
    let mut buckets: Vec<(String, Vec<f64>)> = Vec::new();
    for row in rows.flatten() {
        let (ts, val) = row;
        if let Some(last) = buckets.last_mut().filter(|l| l.0 == ts) {
            last.1.push(val);
            continue;
        }
        buckets.push((ts, vec![val]));
    }

    // Compute per-bucket result
    Ok(buckets
        .into_iter()
        .map(|(ts, mut vals)| {
            let result = match method {
                AggregateMethod::Mid => {
                    // Median: sort and take middle value
                    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let mid = vals.len() / 2;
                    if vals.len() % 2 == 0 && vals.len() >= 2 {
                        (vals[mid - 1] + vals[mid]) / 2.0
                    } else {
                        vals[mid]
                    }
                }
                AggregateMethod::MiddleIndex => {
                    // Value at the middle index (by insertion order = time order)
                    vals[vals.len() / 2]
                }
                _ => unreachable!(),
            };
            (ts, serde_json::json!(result))
        })
        .collect())
}

fn query_daily_values(
    conn: &signalk_sqlite::rusqlite::Connection,
    context: &str,
    path: &str,
    from: &str,
    to: &str,
    method: AggregateMethod,
) -> Result<Vec<(String, serde_json::Value)>, String> {
    let from_date = &from[..10.min(from.len())];
    let to_date = &to[..10.min(to.len())];

    let val_col = match method {
        AggregateMethod::Average | AggregateMethod::Mid | AggregateMethod::MiddleIndex => {
            "avg_value"
        }
        AggregateMethod::Min => "min_value",
        AggregateMethod::Max => "max_value",
        AggregateMethod::Count => "count",
        _ => "avg_value",
    };

    let sql = format!(
        "SELECT date || 'T00:00:00Z', {val_col} FROM history_daily \
         WHERE context = ?1 AND path = ?2 AND date >= ?3 AND date <= ?4 \
         ORDER BY date"
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map(
            signalk_sqlite::rusqlite::params![context, path, from_date, to_date],
            |row| {
                let ts: String = row.get(0)?;
                let val: f64 = row.get(1)?;
                Ok((ts, val))
            },
        )
        .map_err(|e| format!("query: {e}"))?;

    Ok(rows
        .filter_map(|r| r.ok())
        .map(|(ts, val)| (ts, serde_json::json!(val)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::query::PathSpec;
    use super::*;
    use signalk_sqlite::Database;

    fn test_provider() -> SqliteHistoryProvider {
        let db = Database::open_in_memory().unwrap();
        let conn = Arc::new(Mutex::new(db.into_conn()));
        SqliteHistoryProvider::new(conn)
    }

    #[test]
    fn record_and_query_values() {
        let p = test_provider();
        p.record_batch(&[
            (
                "vessels.self".into(),
                "navigation.speedOverGround".into(),
                5.0,
                Some("2026-03-02T12:00:00Z".into()),
            ),
            (
                "vessels.self".into(),
                "navigation.speedOverGround".into(),
                6.0,
                Some("2026-03-02T12:00:01Z".into()),
            ),
            (
                "vessels.self".into(),
                "navigation.speedOverGround".into(),
                7.0,
                Some("2026-03-02T12:00:02Z".into()),
            ),
        ])
        .unwrap();

        let resp = p
            .get_values(&ValuesRequest {
                context: "vessels.self".into(),
                path_specs: vec![PathSpec {
                    path: "navigation.speedOverGround".into(),
                    method: AggregateMethod::Average,
                }],
                from: Some("2026-03-02T12:00:00Z".into()),
                to: Some("2026-03-02T12:00:02Z".into()),
                duration: None,
                resolution: None,
            })
            .unwrap();

        assert_eq!(resp.context, "vessels.self");
        assert_eq!(resp.data.len(), 3);
        assert_eq!(resp.values.len(), 1);
        assert_eq!(resp.values[0].path, "navigation.speedOverGround");
    }

    #[test]
    fn record_and_query_with_resolution() {
        let p = test_provider();
        // Insert data across two minutes
        for i in 0..120 {
            p.record_batch(&[(
                "vessels.self".into(),
                "navigation.speedOverGround".into(),
                5.0 + (i as f64) * 0.01,
                Some(format!("2026-03-02T12:{:02}:{:02}Z", i / 60, i % 60)),
            )])
            .unwrap();
        }

        let resp = p
            .get_values(&ValuesRequest {
                context: "vessels.self".into(),
                path_specs: vec![PathSpec {
                    path: "navigation.speedOverGround".into(),
                    method: AggregateMethod::Average,
                }],
                from: Some("2026-03-02T12:00:00Z".into()),
                to: Some("2026-03-02T12:01:59Z".into()),
                duration: None,
                resolution: Some("1m".into()),
            })
            .unwrap();

        // Should be grouped into 2 minutes
        assert_eq!(
            resp.data.len(),
            2,
            "expected 2 minute buckets, got {}",
            resp.data.len()
        );
    }

    #[test]
    fn query_contexts() {
        let p = test_provider();
        p.record_batch(&[
            (
                "vessels.self".into(),
                "nav.sog".into(),
                5.0,
                Some("2026-03-02T12:00:00Z".into()),
            ),
            (
                "vessels.urn:mrn:imo:mmsi:211234567".into(),
                "nav.sog".into(),
                3.0,
                Some("2026-03-02T12:00:00Z".into()),
            ),
        ])
        .unwrap();

        let contexts = p
            .get_contexts(&ContextsRequest {
                from: Some("2026-03-02T00:00:00Z".into()),
                to: Some("2026-03-02T23:59:59Z".into()),
                duration: None,
            })
            .unwrap();

        assert_eq!(contexts.len(), 2);
        assert!(contexts.contains(&"vessels.self".to_string()));
    }

    #[test]
    fn query_paths() {
        let p = test_provider();
        p.record_batch(&[
            (
                "vessels.self".into(),
                "navigation.speedOverGround".into(),
                5.0,
                Some("2026-03-02T12:00:00Z".into()),
            ),
            (
                "vessels.self".into(),
                "navigation.courseOverGroundTrue".into(),
                1.5,
                Some("2026-03-02T12:00:00Z".into()),
            ),
            (
                "vessels.self".into(),
                "environment.wind.speedApparent".into(),
                8.0,
                Some("2026-03-02T12:00:00Z".into()),
            ),
        ])
        .unwrap();

        let paths = p
            .get_paths(&PathsRequest {
                context: "vessels.self".into(),
                from: Some("2026-03-02T00:00:00Z".into()),
                to: Some("2026-03-02T23:59:59Z".into()),
                duration: None,
            })
            .unwrap();

        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&"navigation.speedOverGround".to_string()));
    }

    #[test]
    fn aggregate_and_prune() {
        let p = test_provider();
        // Insert "old" data (we fake it by inserting timestamps 10 days ago)
        p.record_batch(&[
            (
                "vessels.self".into(),
                "nav.sog".into(),
                5.0,
                Some("2020-01-01T12:00:00Z".into()),
            ),
            (
                "vessels.self".into(),
                "nav.sog".into(),
                7.0,
                Some("2020-01-01T12:00:01Z".into()),
            ),
            (
                "vessels.self".into(),
                "nav.sog".into(),
                9.0,
                Some("2020-01-01T13:00:00Z".into()),
            ),
        ])
        .unwrap();

        // Aggregate with 0 days retention for raw (force aggregation)
        p.aggregate_and_prune(0, 36500).unwrap();

        // Raw should be empty
        let conn = p.conn.lock().unwrap();
        let raw_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_raw", [], |row| row.get(0))
            .unwrap();
        assert_eq!(raw_count, 0);

        // Daily should have aggregated data
        let daily_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_daily", [], |row| row.get(0))
            .unwrap();
        assert!(daily_count > 0, "expected daily data after aggregation");

        // Verify aggregation values
        let (avg, min, max, count): (f64, f64, f64, i64) = conn
            .query_row(
                "SELECT avg_value, min_value, max_value, count FROM history_daily \
             WHERE context = 'vessels.self' AND path = 'nav.sog'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!((avg - 7.0).abs() < 0.01, "avg should be 7.0, got {avg}");
        assert!((min - 5.0).abs() < 0.01);
        assert!((max - 9.0).abs() < 0.01);
        assert_eq!(count, 3);
    }

    #[test]
    fn empty_query_returns_empty() {
        let p = test_provider();
        let resp = p
            .get_values(&ValuesRequest {
                context: "vessels.self".into(),
                path_specs: vec![PathSpec {
                    path: "nav.sog".into(),
                    method: AggregateMethod::Average,
                }],
                from: Some("2026-03-02T12:00:00Z".into()),
                to: Some("2026-03-02T13:00:00Z".into()),
                duration: None,
                resolution: None,
            })
            .unwrap();
        assert!(resp.data.is_empty());
    }

    #[test]
    fn resolve_time_range_duration_only() {
        let (from, to) = resolve_time_range(&None, &None, &Some("PT1H".into())).unwrap();
        assert!(!from.is_empty());
        assert!(!to.is_empty());
        // from should be about 1 hour before to
        let from_dt = chrono::DateTime::parse_from_rfc3339(&from).unwrap();
        let to_dt = chrono::DateTime::parse_from_rfc3339(&to).unwrap();
        let diff = (to_dt - from_dt).num_seconds();
        assert!((diff - 3600).abs() < 2, "expected ~3600s, got {diff}");
    }

    #[test]
    fn resolve_time_range_from_to() {
        let (from, to) = resolve_time_range(
            &Some("2026-03-02T12:00:00Z".into()),
            &Some("2026-03-02T13:00:00Z".into()),
            &None,
        )
        .unwrap();
        assert_eq!(from, "2026-03-02T12:00:00Z");
        assert_eq!(to, "2026-03-02T13:00:00Z");
    }

    #[test]
    fn db_size_bytes_positive_after_insert() {
        let p = test_provider();
        p.record_batch(&[(
            "vessels.self".into(),
            "nav.sog".into(),
            5.0,
            Some("2026-03-02T12:00:00Z".into()),
        )])
        .unwrap();
        let size = p.db_size_bytes().unwrap();
        assert!(size > 0, "expected positive DB size, got {size}");
    }

    #[test]
    fn query_nonexistent_path_returns_empty() {
        let p = test_provider();
        // Insert data for path A
        p.record_batch(&[(
            "vessels.self".into(),
            "navigation.speedOverGround".into(),
            5.0,
            Some("2026-03-02T12:00:00Z".into()),
        )])
        .unwrap();

        // Query path B — should return empty, not error
        let resp = p
            .get_values(&ValuesRequest {
                context: "vessels.self".into(),
                path_specs: vec![PathSpec {
                    path: "navigation.courseOverGroundTrue".into(),
                    method: AggregateMethod::Average,
                }],
                from: Some("2026-03-02T12:00:00Z".into()),
                to: Some("2026-03-02T13:00:00Z".into()),
                duration: None,
                resolution: None,
            })
            .unwrap();
        assert!(resp.data.is_empty());
    }

    #[test]
    fn record_high_frequency_values() {
        let p = test_provider();
        let mut batch: Vec<(String, String, f64, Option<String>)> = Vec::new();
        // 1000 values at 1-second intervals: 12:00:00 to 12:16:39
        for i in 0..1000u32 {
            let minutes = i / 60;
            let secs = i % 60;
            batch.push((
                "vessels.self".into(),
                "navigation.speedOverGround".into(),
                5.0 + (i as f64) * 0.001,
                Some(format!("2026-03-02T12:{minutes:02}:{secs:02}Z")),
            ));
        }
        p.record_batch(&batch).unwrap();

        let resp = p
            .get_values(&ValuesRequest {
                context: "vessels.self".into(),
                path_specs: vec![PathSpec {
                    path: "navigation.speedOverGround".into(),
                    method: AggregateMethod::Average,
                }],
                from: Some("2026-03-02T12:00:00Z".into()),
                to: Some("2026-03-02T12:59:59Z".into()),
                duration: None,
                resolution: None,
            })
            .unwrap();

        assert_eq!(resp.data.len(), 1000, "all 1000 values should be returned");
        // Verify ordering: first value < last value
        let first = resp.data[0][1].as_f64().unwrap();
        let last = resp.data[999][1].as_f64().unwrap();
        assert!(first < last, "values should be in chronological order");
    }

    #[test]
    fn vacuum_succeeds() {
        let p = test_provider();
        p.record_batch(&[(
            "vessels.self".into(),
            "nav.sog".into(),
            5.0,
            Some("2020-01-01T12:00:00Z".into()),
        )])
        .unwrap();
        p.aggregate_and_prune(0, 0).unwrap();
        p.vacuum().unwrap();
    }
}
