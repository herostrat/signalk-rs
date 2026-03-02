//! Track data types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single recorded track point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackPoint {
    pub lat: f64,
    pub lon: f64,
    pub timestamp: DateTime<Utc>,
    /// Speed over ground in m/s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sog: Option<f64>,
    /// Course over ground true in radians.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cog: Option<f64>,
    /// Water depth below transducer in meters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<f64>,
}

/// A contiguous segment of track points (split at time gaps).
#[derive(Debug, Clone)]
pub struct TrackSegment {
    pub points: Vec<TrackPoint>,
}

/// Complete track data for a single vessel.
#[derive(Debug, Clone)]
pub struct VesselTrack {
    /// SignalK context, e.g. `"vessels.self"` or `"vessels.urn:mrn:imo:mmsi:..."`.
    pub context: String,
    /// Display label (vessel name if known).
    pub label: Option<String>,
    /// Ordered segments (newest point last within each segment).
    pub segments: Vec<TrackSegment>,
}

/// Summary information for a single vessel's track.
#[derive(Debug, Clone, Serialize)]
pub struct TrackSummary {
    pub context: String,
    pub point_count: usize,
    pub oldest: Option<DateTime<Utc>>,
    pub newest: Option<DateTime<Utc>>,
}

/// Query parameters for track retrieval.
#[derive(Debug, Clone, Default)]
pub struct TrackQuery {
    /// Filter to a specific vessel context.
    pub context: Option<String>,
    /// Only return points after this time.
    pub after: Option<DateTime<Utc>>,
    /// Only return points before this time.
    pub before: Option<DateTime<Utc>>,
    /// Bounding box: (west, south, east, north).
    pub bbox: Option<(f64, f64, f64, f64)>,
    /// Radius filter: (center_lat, center_lon, meters).
    pub radius: Option<(f64, f64, f64)>,
    /// Maximum number of points to return per vessel.
    pub limit: Option<usize>,
    /// Douglas-Peucker simplification tolerance in degrees.
    /// `None` = no simplification. ~0.00001° ≈ 1 m at equator.
    pub simplify: Option<f64>,
    /// Output format.
    pub format: TrackFormat,
}

/// Output format for track API responses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TrackFormat {
    #[default]
    GeoJson,
    Gpx,
}
