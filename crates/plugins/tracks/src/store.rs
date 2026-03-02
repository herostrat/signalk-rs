//! SQLite-backed track storage using the shared `track_points` table.

use crate::types::{TrackPoint, TrackQuery, TrackSegment, TrackSummary, VesselTrack};
use chrono::{TimeDelta, Utc};
use signalk_sqlite::rusqlite::Connection;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Gap duration that splits a track into separate segments.
const SEGMENT_GAP: TimeDelta = TimeDelta::minutes(5);

/// SQLite-backed track store using the `track_points` table.
///
/// Uses a shared `Arc<Mutex<Connection>>` — all locking is internal,
/// so callers can use `&self` (no outer Mutex needed).
pub struct SqliteTrackStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteTrackStore {
    /// Create a new SQLite track store from a shared connection.
    ///
    /// The connection must already have the `track_points` table (created by
    /// `signalk_sqlite::Database::migrate()`).
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        SqliteTrackStore { conn }
    }

    /// Record a new track point for a vessel.
    pub fn record(&self, context: &str, point: TrackPoint) {
        let conn = self.conn.lock().unwrap();
        let ts = point.timestamp.to_rfc3339();
        let _ = conn.execute(
            "INSERT INTO track_points (context, lat, lon, timestamp, sog, cog, depth) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            signalk_sqlite::rusqlite::params![
                context,
                point.lat,
                point.lon,
                ts,
                point.sog,
                point.cog,
                point.depth
            ],
        );
    }

    /// Query tracks matching the given filters.
    pub fn query(&self, query: &TrackQuery) -> Vec<VesselTrack> {
        let conn = self.conn.lock().unwrap();

        // Build SQL dynamically with filters
        let mut sql = String::from(
            "SELECT context, lat, lon, timestamp, sog, cog, depth FROM track_points WHERE 1=1",
        );
        let mut params: Vec<Box<dyn signalk_sqlite::rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref ctx) = query.context {
            sql.push_str(&format!(" AND context = ?{}", params.len() + 1));
            params.push(Box::new(ctx.clone()));
        }
        if let Some(ref after) = query.after {
            sql.push_str(&format!(" AND timestamp >= ?{}", params.len() + 1));
            params.push(Box::new(after.to_rfc3339()));
        }
        if let Some(ref before) = query.before {
            sql.push_str(&format!(" AND timestamp <= ?{}", params.len() + 1));
            params.push(Box::new(before.to_rfc3339()));
        }
        if let Some((west, south, east, north)) = query.bbox {
            sql.push_str(&format!(
                " AND lon >= ?{} AND lon <= ?{} AND lat >= ?{} AND lat <= ?{}",
                params.len() + 1,
                params.len() + 2,
                params.len() + 3,
                params.len() + 4,
            ));
            params.push(Box::new(west));
            params.push(Box::new(east));
            params.push(Box::new(south));
            params.push(Box::new(north));
        }

        sql.push_str(" ORDER BY context, timestamp");

        let param_refs: Vec<&dyn signalk_sqlite::rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let rows = match stmt.query_map(param_refs.as_slice(), |row| {
            let ts_str: String = row.get(3)?;
            let timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_default();
            Ok((
                row.get::<_, String>(0)?,
                TrackPoint {
                    lat: row.get(1)?,
                    lon: row.get(2)?,
                    timestamp,
                    sog: row.get(4)?,
                    cog: row.get(5)?,
                    depth: row.get(6)?,
                },
            ))
        }) {
            Ok(r) => r,
            Err(_) => return vec![],
        };

        // Group by context, apply radius filter
        let mut grouped: HashMap<String, Vec<TrackPoint>> = HashMap::new();
        for row in rows.flatten() {
            let (ctx, pt) = row;
            // Radius filter (haversine in Rust)
            if let Some((center_lat, center_lon, radius_m)) = query.radius {
                let dist =
                    signalk_types::geo::haversine_meters(center_lat, center_lon, pt.lat, pt.lon);
                if dist > radius_m {
                    continue;
                }
            }
            grouped.entry(ctx).or_default().push(pt);
        }

        grouped
            .into_iter()
            .filter_map(|(context, points)| {
                if points.is_empty() {
                    return None;
                }
                // Apply limit (take most recent N points)
                let limited = if let Some(limit) = query.limit {
                    if points.len() > limit {
                        points[points.len() - limit..].to_vec()
                    } else {
                        points
                    }
                } else {
                    points
                };

                let segments = segment_by_gap(&limited);
                Some(VesselTrack {
                    context,
                    label: None,
                    segments,
                })
            })
            .collect()
    }

    /// Get a summary of tracked vessels.
    pub fn summary(&self) -> Vec<TrackSummary> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT context, COUNT(*), MIN(timestamp), MAX(timestamp) \
                 FROM track_points GROUP BY context",
            )
            .unwrap();
        stmt.query_map([], |row| {
            let min_ts: Option<String> = row.get(2)?;
            let max_ts: Option<String> = row.get(3)?;
            Ok(TrackSummary {
                context: row.get(0)?,
                point_count: row.get::<_, usize>(1)?,
                oldest: min_ts.and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|dt| dt.with_timezone(&Utc))
                }),
                newest: max_ts.and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|dt| dt.with_timezone(&Utc))
                }),
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    /// Prune old data beyond max_age.
    pub fn prune(&self, max_age: TimeDelta) {
        let conn = self.conn.lock().unwrap();
        let cutoff = (Utc::now() - max_age).to_rfc3339();
        let _ = conn.execute("DELETE FROM track_points WHERE timestamp < ?1", [&cutoff]);
    }

    /// Total number of stored points across all vessels.
    pub fn total_points(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM track_points", [], |row| {
            row.get::<_, usize>(0)
        })
        .unwrap_or(0)
    }

    /// Number of tracked vessels.
    pub fn vessel_count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(DISTINCT context) FROM track_points",
            [],
            |row| row.get::<_, usize>(0),
        )
        .unwrap_or(0)
    }

    /// Delete all track data for a specific vessel.
    pub fn clear_vessel(&self, context: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM track_points WHERE context = ?1", [context]);
    }

    /// Delete all track data.
    pub fn clear_all(&self) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM track_points", []);
    }
}

/// Split a sorted list of points into segments at time gaps > SEGMENT_GAP.
fn segment_by_gap(points: &[TrackPoint]) -> Vec<TrackSegment> {
    if points.is_empty() {
        return vec![];
    }
    let mut segments = vec![];
    let mut current = vec![points[0].clone()];

    for window in points.windows(2) {
        let gap = window[1].timestamp - window[0].timestamp;
        if gap > SEGMENT_GAP {
            segments.push(TrackSegment { points: current });
            current = vec![];
        }
        current.push(window[1].clone());
    }
    if !current.is_empty() {
        segments.push(TrackSegment { points: current });
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Duration};

    fn point(lat: f64, lon: f64, minutes_ago: i64) -> TrackPoint {
        TrackPoint {
            lat,
            lon,
            timestamp: Utc::now() - Duration::minutes(minutes_ago),
            sog: Some(3.0),
            cog: Some(1.57),
            depth: Some(10.0),
        }
    }

    fn point_at(lat: f64, lon: f64, ts: DateTime<Utc>) -> TrackPoint {
        TrackPoint {
            lat,
            lon,
            timestamp: ts,
            sog: None,
            cog: None,
            depth: None,
        }
    }

    fn sqlite_store() -> SqliteTrackStore {
        let db = signalk_sqlite::Database::open_in_memory().unwrap();
        SqliteTrackStore::new(Arc::new(Mutex::new(db.into_conn())))
    }

    #[test]
    fn record_and_query_roundtrip() {
        let store = sqlite_store();
        store.record("vessels.self", point(54.0, 10.0, 2));
        store.record("vessels.self", point(54.1, 10.1, 1));

        let tracks = store.query(&TrackQuery::default());
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].context, "vessels.self");
        let total: usize = tracks[0].segments.iter().map(|s| s.points.len()).sum();
        assert_eq!(total, 2);
    }

    #[test]
    fn query_time_filter() {
        let store = sqlite_store();
        let now = Utc::now();
        store.record("v", point_at(54.0, 10.0, now - Duration::hours(3)));
        store.record("v", point_at(54.1, 10.1, now - Duration::hours(1)));
        store.record("v", point_at(54.2, 10.2, now));

        let tracks = store.query(&TrackQuery {
            after: Some(now - Duration::hours(2)),
            ..Default::default()
        });
        let pts: Vec<_> = tracks[0].segments.iter().flat_map(|s| &s.points).collect();
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn query_bbox_filter() {
        let store = sqlite_store();
        store.record("v", point(54.0, 10.0, 2)); // inside
        store.record("v", point(55.0, 10.0, 1)); // outside (north)
        store.record("v", point(54.5, 10.5, 0)); // inside

        let tracks = store.query(&TrackQuery {
            bbox: Some((9.0, 53.5, 11.0, 54.8)),
            ..Default::default()
        });
        let pts: Vec<_> = tracks[0].segments.iter().flat_map(|s| &s.points).collect();
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn query_radius_filter() {
        let store = sqlite_store();
        store.record("v", point(53.55, 10.0, 2)); // ~0m from center
        store.record("v", point(54.55, 10.0, 1)); // ~111km away

        let tracks = store.query(&TrackQuery {
            radius: Some((53.55, 10.0, 50_000.0)),
            ..Default::default()
        });
        let pts: Vec<_> = tracks[0].segments.iter().flat_map(|s| &s.points).collect();
        assert_eq!(pts.len(), 1);
        assert!((pts[0].lat - 53.55).abs() < 0.01);
    }

    #[test]
    fn query_limit() {
        let store = sqlite_store();
        for i in 0..10 {
            store.record("v", point(54.0, 10.0 + i as f64 * 0.01, 10 - i));
        }

        let tracks = store.query(&TrackQuery {
            limit: Some(3),
            ..Default::default()
        });
        let pts: Vec<_> = tracks[0].segments.iter().flat_map(|s| &s.points).collect();
        assert_eq!(pts.len(), 3);
    }

    #[test]
    fn query_single_vessel() {
        let store = sqlite_store();
        store.record("v1", point(54.0, 10.0, 1));
        store.record("v2", point(55.0, 11.0, 1));

        let tracks = store.query(&TrackQuery {
            context: Some("v1".into()),
            ..Default::default()
        });
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].context, "v1");
    }

    #[test]
    fn query_all_vessels() {
        let store = sqlite_store();
        store.record("v1", point(54.0, 10.0, 1));
        store.record("v2", point(55.0, 11.0, 1));

        let tracks = store.query(&TrackQuery::default());
        assert_eq!(tracks.len(), 2);
    }

    #[test]
    fn prune_removes_old_points() {
        let store = sqlite_store();
        let now = Utc::now();
        store.record("v", point_at(54.0, 10.0, now - Duration::hours(25)));
        store.record("v", point_at(54.1, 10.1, now));

        store.prune(Duration::hours(24));
        assert_eq!(store.total_points(), 1);
    }

    #[test]
    fn prune_removes_empty_vessels() {
        let store = sqlite_store();
        let now = Utc::now();
        store.record("v", point_at(54.0, 10.0, now - Duration::hours(25)));

        store.prune(Duration::hours(24));
        assert_eq!(store.vessel_count(), 0);
    }

    #[test]
    fn clear_vessel() {
        let store = sqlite_store();
        store.record("v1", point(54.0, 10.0, 1));
        store.record("v2", point(55.0, 11.0, 1));

        store.clear_vessel("v1");
        assert_eq!(store.vessel_count(), 1);
        assert_eq!(store.total_points(), 1);
    }

    #[test]
    fn clear_all() {
        let store = sqlite_store();
        store.record("v1", point(54.0, 10.0, 1));
        store.record("v2", point(55.0, 11.0, 1));

        store.clear_all();
        assert_eq!(store.vessel_count(), 0);
        assert_eq!(store.total_points(), 0);
    }

    #[test]
    fn summary() {
        let store = sqlite_store();
        store.record("v1", point(54.0, 10.0, 2));
        store.record("v1", point(54.1, 10.1, 1));
        store.record("v2", point(55.0, 11.0, 1));

        let mut summary = store.summary();
        summary.sort_by(|a, b| a.context.cmp(&b.context));
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0].context, "v1");
        assert_eq!(summary[0].point_count, 2);
        assert_eq!(summary[1].context, "v2");
        assert_eq!(summary[1].point_count, 1);
    }

    #[test]
    fn segment_by_gap_single_segment() {
        let now = Utc::now();
        let points = vec![
            point_at(54.0, 10.0, now),
            point_at(54.1, 10.1, now + Duration::minutes(1)),
            point_at(54.2, 10.2, now + Duration::minutes(2)),
        ];
        let segments = segment_by_gap(&points);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].points.len(), 3);
    }

    #[test]
    fn segment_by_gap_splits_at_gap() {
        let now = Utc::now();
        let points = vec![
            point_at(54.0, 10.0, now),
            point_at(54.1, 10.1, now + Duration::minutes(1)),
            // 10 minute gap here
            point_at(54.2, 10.2, now + Duration::minutes(11)),
            point_at(54.3, 10.3, now + Duration::minutes(12)),
        ];
        let segments = segment_by_gap(&points);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].points.len(), 2);
        assert_eq!(segments[1].points.len(), 2);
    }

    #[test]
    fn segment_by_gap_empty() {
        let segments = segment_by_gap(&[]);
        assert!(segments.is_empty());
    }

    #[test]
    fn segment_by_gap_in_query() {
        let store = sqlite_store();
        let now = Utc::now();
        store.record("v", point_at(54.0, 10.0, now));
        store.record("v", point_at(54.1, 10.1, now + Duration::minutes(1)));
        // 10 minute gap
        store.record("v", point_at(54.2, 10.2, now + Duration::minutes(11)));

        let tracks = store.query(&TrackQuery {
            context: Some("v".into()),
            ..Default::default()
        });
        assert_eq!(tracks[0].segments.len(), 2);
    }
}
