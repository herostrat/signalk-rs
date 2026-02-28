//! Track storage trait and in-memory implementation.

use crate::types::{TrackPoint, TrackQuery, TrackSegment, TrackSummary, VesselTrack};
use chrono::{TimeDelta, Utc};
use std::collections::{HashMap, VecDeque};

/// Gap duration that splits a track into separate segments.
const SEGMENT_GAP: TimeDelta = TimeDelta::minutes(5);

/// Abstraction over track storage backends.
///
/// All methods are synchronous — the plugin holds the store behind
/// `Arc<Mutex<dyn TrackStore>>`, matching the AIS tracker pattern.
pub trait TrackStore: Send + 'static {
    /// Record a new track point for a vessel.
    fn record(&mut self, context: &str, point: TrackPoint);

    /// Query tracks matching the given filters.
    fn query(&self, query: &TrackQuery) -> Vec<VesselTrack>;

    /// Get a summary of tracked vessels.
    fn summary(&self) -> Vec<TrackSummary>;

    /// Prune old data beyond max_age. Called periodically by the tick task.
    fn prune(&mut self, max_age: TimeDelta);

    /// Total number of stored points across all vessels.
    fn total_points(&self) -> usize;

    /// Number of tracked vessels.
    fn vessel_count(&self) -> usize;

    /// Delete all track data for a specific vessel.
    fn clear_vessel(&mut self, context: &str);

    /// Delete all track data.
    fn clear_all(&mut self);
}

/// In-memory track store using a VecDeque ring buffer per vessel.
pub struct InMemoryTrackStore {
    tracks: HashMap<String, VecDeque<TrackPoint>>,
    capacity: usize,
}

impl InMemoryTrackStore {
    pub fn new(capacity: usize) -> Self {
        InMemoryTrackStore {
            tracks: HashMap::new(),
            capacity,
        }
    }
}

impl TrackStore for InMemoryTrackStore {
    fn record(&mut self, context: &str, point: TrackPoint) {
        let deque = self
            .tracks
            .entry(context.to_string())
            .or_insert_with(|| VecDeque::with_capacity(self.capacity.min(1024)));

        if deque.len() >= self.capacity {
            deque.pop_front();
        }
        deque.push_back(point);
    }

    fn query(&self, query: &TrackQuery) -> Vec<VesselTrack> {
        let iter: Box<dyn Iterator<Item = (&String, &VecDeque<TrackPoint>)>> =
            if let Some(ref ctx) = query.context {
                Box::new(self.tracks.get(ctx).into_iter().map(move |d| (ctx, d)))
            } else {
                Box::new(self.tracks.iter())
            };

        iter.filter_map(|(context, points)| {
            let filtered: Vec<TrackPoint> = points
                .iter()
                .filter(|p| {
                    if let Some(ref after) = query.after
                        && p.timestamp < *after
                    {
                        return false;
                    }
                    if let Some(ref before) = query.before
                        && p.timestamp > *before
                    {
                        return false;
                    }
                    if let Some((west, south, east, north)) = query.bbox
                        && (p.lon < west || p.lon > east || p.lat < south || p.lat > north)
                    {
                        return false;
                    }
                    if let Some((center_lat, center_lon, radius_m)) = query.radius {
                        let dist = signalk_types::geo::haversine_meters(
                            center_lat, center_lon, p.lat, p.lon,
                        );
                        if dist > radius_m {
                            return false;
                        }
                    }
                    true
                })
                .cloned()
                .collect();

            if filtered.is_empty() {
                return None;
            }

            // Apply limit (take most recent N points)
            let limited = if let Some(limit) = query.limit {
                if filtered.len() > limit {
                    filtered[filtered.len() - limit..].to_vec()
                } else {
                    filtered
                }
            } else {
                filtered
            };

            let segments = segment_by_gap(&limited);

            Some(VesselTrack {
                context: context.clone(),
                label: None,
                segments,
            })
        })
        .collect()
    }

    fn summary(&self) -> Vec<TrackSummary> {
        self.tracks
            .iter()
            .map(|(context, points)| TrackSummary {
                context: context.clone(),
                point_count: points.len(),
                oldest: points.front().map(|p| p.timestamp),
                newest: points.back().map(|p| p.timestamp),
            })
            .collect()
    }

    fn prune(&mut self, max_age: TimeDelta) {
        let cutoff = Utc::now() - max_age;
        for deque in self.tracks.values_mut() {
            while deque.front().is_some_and(|p| p.timestamp < cutoff) {
                deque.pop_front();
            }
        }
        self.tracks.retain(|_, d| !d.is_empty());
    }

    fn total_points(&self) -> usize {
        self.tracks.values().map(|d| d.len()).sum()
    }

    fn vessel_count(&self) -> usize {
        self.tracks.len()
    }

    fn clear_vessel(&mut self, context: &str) {
        self.tracks.remove(context);
    }

    fn clear_all(&mut self) {
        self.tracks.clear();
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

    #[test]
    fn record_and_query_roundtrip() {
        let mut store = InMemoryTrackStore::new(1000);
        store.record("vessels.self", point(54.0, 10.0, 2));
        store.record("vessels.self", point(54.1, 10.1, 1));

        let tracks = store.query(&TrackQuery::default());
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].context, "vessels.self");
        let total_points: usize = tracks[0].segments.iter().map(|s| s.points.len()).sum();
        assert_eq!(total_points, 2);
    }

    #[test]
    fn capacity_eviction() {
        let mut store = InMemoryTrackStore::new(3);
        for i in 0..5 {
            store.record("v", point(54.0 + i as f64 * 0.01, 10.0, 10 - i));
        }
        assert_eq!(store.total_points(), 3);

        let tracks = store.query(&TrackQuery {
            context: Some("v".into()),
            ..Default::default()
        });
        // Should have the 3 most recent points
        let pts: Vec<_> = tracks[0].segments.iter().flat_map(|s| &s.points).collect();
        assert_eq!(pts.len(), 3);
        // Oldest remaining should be point at index 2 (minutes_ago = 8)
        assert!((pts[0].lat - 54.02).abs() < 0.001);
    }

    #[test]
    fn query_time_filter() {
        let mut store = InMemoryTrackStore::new(1000);
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
        let mut store = InMemoryTrackStore::new(1000);
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
        let mut store = InMemoryTrackStore::new(1000);
        // Hamburg: 53.55, 10.0
        store.record("v", point(53.55, 10.0, 2)); // ~0m from center
        store.record("v", point(54.55, 10.0, 1)); // ~111km away

        let tracks = store.query(&TrackQuery {
            radius: Some((53.55, 10.0, 50_000.0)), // 50km radius
            ..Default::default()
        });
        let pts: Vec<_> = tracks[0].segments.iter().flat_map(|s| &s.points).collect();
        assert_eq!(pts.len(), 1);
        assert!((pts[0].lat - 53.55).abs() < 0.01);
    }

    #[test]
    fn query_limit() {
        let mut store = InMemoryTrackStore::new(1000);
        for i in 0..10 {
            store.record("v", point(54.0, 10.0 + i as f64 * 0.01, 10 - i));
        }

        let tracks = store.query(&TrackQuery {
            limit: Some(3),
            ..Default::default()
        });
        let pts: Vec<_> = tracks[0].segments.iter().flat_map(|s| &s.points).collect();
        assert_eq!(pts.len(), 3);
        // Should be the 3 most recent
        assert!((pts[2].lon - 10.09).abs() < 0.001);
    }

    #[test]
    fn query_single_vessel() {
        let mut store = InMemoryTrackStore::new(1000);
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
        let mut store = InMemoryTrackStore::new(1000);
        store.record("v1", point(54.0, 10.0, 1));
        store.record("v2", point(55.0, 11.0, 1));

        let tracks = store.query(&TrackQuery::default());
        assert_eq!(tracks.len(), 2);
    }

    #[test]
    fn prune_removes_old_points() {
        let mut store = InMemoryTrackStore::new(1000);
        let now = Utc::now();
        store.record("v", point_at(54.0, 10.0, now - Duration::hours(25)));
        store.record("v", point_at(54.1, 10.1, now));

        store.prune(Duration::hours(24));
        assert_eq!(store.total_points(), 1);
    }

    #[test]
    fn prune_removes_empty_vessels() {
        let mut store = InMemoryTrackStore::new(1000);
        let now = Utc::now();
        store.record("v", point_at(54.0, 10.0, now - Duration::hours(25)));

        store.prune(Duration::hours(24));
        assert_eq!(store.vessel_count(), 0);
    }

    #[test]
    fn clear_vessel() {
        let mut store = InMemoryTrackStore::new(1000);
        store.record("v1", point(54.0, 10.0, 1));
        store.record("v2", point(55.0, 11.0, 1));

        store.clear_vessel("v1");
        assert_eq!(store.vessel_count(), 1);
        assert_eq!(store.total_points(), 1);
    }

    #[test]
    fn clear_all() {
        let mut store = InMemoryTrackStore::new(1000);
        store.record("v1", point(54.0, 10.0, 1));
        store.record("v2", point(55.0, 11.0, 1));

        store.clear_all();
        assert_eq!(store.vessel_count(), 0);
        assert_eq!(store.total_points(), 0);
    }

    #[test]
    fn summary() {
        let mut store = InMemoryTrackStore::new(1000);
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
}
