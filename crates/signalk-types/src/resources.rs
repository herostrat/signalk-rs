/// SignalK resource and course domain types.
///
/// Defines the core data structures for the Resources API and Course API:
/// - `Position` — geographic coordinates
/// - `ResourceType` — standard resource types (waypoints, routes, etc.)
/// - `CourseState` — active navigation state
/// - `ActiveRoute`, `CoursePoint`, `PointType` — course sub-structures
use serde::{Deserialize, Serialize};

// ─── Position ──────────────────────────────────────────────────────────────────

/// A geographic position in WGS84.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altitude: Option<f64>,
}

// ─── Resource Types ────────────────────────────────────────────────────────────

/// Standard resource types defined by the SignalK specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    Waypoints,
    Routes,
    Notes,
    Regions,
    Charts,
}

impl ResourceType {
    /// All standard resource types.
    pub const ALL: &[ResourceType] = &[
        ResourceType::Waypoints,
        ResourceType::Routes,
        ResourceType::Notes,
        ResourceType::Regions,
        ResourceType::Charts,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Waypoints => "waypoints",
            Self::Routes => "routes",
            Self::Notes => "notes",
            Self::Regions => "regions",
            Self::Charts => "charts",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "waypoints" => Some(Self::Waypoints),
            "routes" => Some(Self::Routes),
            "notes" => Some(Self::Notes),
            "regions" => Some(Self::Regions),
            "charts" => Some(Self::Charts),
            _ => None,
        }
    }
}

impl std::fmt::Display for ResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─── Course State ──────────────────────────────────────────────────────────────

/// The full course/navigation state.
///
/// Persisted to disk and emitted as SignalK deltas under
/// `navigation.courseGreatCircle.*`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseState {
    /// When navigation was started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,

    /// Arrival circle radius in meters.
    #[serde(default)]
    pub arrival_circle: f64,

    /// Active route being followed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_route: Option<ActiveRoute>,

    /// The next point we are navigating toward.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_point: Option<CoursePoint>,

    /// The point we came from (for XTE calculation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_point: Option<CoursePoint>,
}

/// Reference to an active route being followed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveRoute {
    /// Resource reference (e.g., `/resources/routes/{id}`).
    pub href: String,

    /// Whether the route is being navigated in reverse.
    #[serde(default)]
    pub reverse: bool,

    /// Current waypoint index in the route.
    #[serde(default)]
    pub point_index: usize,

    /// Total number of waypoints in the route.
    ///
    /// Stored at route-set / advance time so arrival detection can determine
    /// whether more waypoints remain without an async resource lookup.
    #[serde(default)]
    pub point_total: usize,

    /// Route name (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A point in the course (next destination or previous waypoint).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoursePoint {
    /// Whether this is a direct destination or a route waypoint.
    #[serde(rename = "type")]
    pub type_: PointType,

    /// Geographic position.
    pub position: Position,

    /// Resource reference (if this point came from a resource).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

/// Type of a course point.
///
/// Serializes as lowercase per SignalK spec (consistent with all other enum fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PointType {
    /// A directly set destination (lat/lon).
    Destination,
    /// A waypoint from a route or resource.
    Waypoint,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_roundtrip() {
        let pos = Position {
            latitude: 49.27,
            longitude: -123.19,
            altitude: None,
        };
        let json = serde_json::to_value(&pos).unwrap();
        assert_eq!(json["latitude"], 49.27);
        assert!(json.get("altitude").is_none());
        let back: Position = serde_json::from_value(json).unwrap();
        assert_eq!(back, pos);
    }

    #[test]
    fn position_with_altitude() {
        let json = serde_json::json!({"latitude": 49.0, "longitude": -123.0, "altitude": 100.5});
        let pos: Position = serde_json::from_value(json).unwrap();
        assert_eq!(pos.altitude, Some(100.5));
    }

    #[test]
    fn resource_type_roundtrip() {
        for rt in ResourceType::ALL {
            let s = rt.as_str();
            assert_eq!(ResourceType::parse(s), Some(*rt));
        }
        assert_eq!(ResourceType::parse("unknown"), None);
    }

    #[test]
    fn resource_type_serde() {
        let json = serde_json::json!("waypoints");
        let rt: ResourceType = serde_json::from_value(json).unwrap();
        assert_eq!(rt, ResourceType::Waypoints);
    }

    #[test]
    fn course_state_roundtrip() {
        let state = CourseState {
            start_time: Some("2026-02-27T12:00:00Z".into()),
            arrival_circle: 50.0,
            active_route: None,
            next_point: Some(CoursePoint {
                type_: PointType::Destination,
                position: Position {
                    latitude: 49.27,
                    longitude: -123.19,
                    altitude: None,
                },
                href: None,
            }),
            previous_point: None,
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["nextPoint"]["type"], "destination");
        let back: CourseState = serde_json::from_value(json).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn active_route_defaults() {
        let json = serde_json::json!({"href": "/resources/routes/abc"});
        let route: ActiveRoute = serde_json::from_value(json).unwrap();
        assert!(!route.reverse);
        assert_eq!(route.point_index, 0);
    }

    #[test]
    fn point_type_serde() {
        let json = serde_json::to_value(PointType::Waypoint).unwrap();
        assert_eq!(json, "waypoint");
        let back: PointType = serde_json::from_value(json).unwrap();
        assert_eq!(back, PointType::Waypoint);
        assert_eq!(
            serde_json::to_value(PointType::Destination).unwrap(),
            "destination"
        );
    }
}
