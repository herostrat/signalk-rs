/// SignalK v2 REST API types.
///
/// All request/response types for the v2 API surface:
/// - Features discovery
/// - Resources CRUD
/// - Course navigation
use serde::{Deserialize, Serialize};

use crate::resources::Position;

// ─── Features API ──────────────────────────────────────────────────────────────

/// Response for `GET /signalk/v2/features`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturesResponse {
    pub apis: Vec<FeatureInfo>,
    pub plugins: Vec<FeatureInfo>,
}

/// A single feature (API or plugin) in the features response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureInfo {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

// ─── Resources API ─────────────────────────────────────────────────────────────

/// Response for `POST /signalk/v2/api/resources/{type}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceResponse {
    pub state: String,
    pub status_code: u16,
    pub id: String,
}

/// Query parameters for `GET /signalk/v2/api/resources/{type}`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResourceQueryParams {
    /// Max distance in meters from vessel position.
    pub distance: Option<f64>,
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Bounding box as `west,south,east,north`.
    pub bbox: Option<String>,
    /// Target a specific provider plugin by ID.
    pub provider: Option<String>,
}

// ─── Course API ────────────────────────────────────────────────────────────────

/// Request body for `PUT .../course/destination`.
#[derive(Debug, Clone, Deserialize)]
pub struct DestinationRequest {
    /// Direct position to navigate to.
    pub position: Option<Position>,
    /// Reference to a waypoint resource (e.g., `/resources/waypoints/{id}`).
    pub href: Option<String>,
}

/// Request body for `PUT .../course/activeRoute`.
#[derive(Debug, Clone, Deserialize)]
pub struct ActiveRouteRequest {
    /// Reference to a route resource (e.g., `/resources/routes/{id}`).
    pub href: String,
    /// Navigate in reverse direction.
    #[serde(default)]
    pub reverse: bool,
}

/// Request body for `PUT .../course/activeRoute/nextPoint`.
#[derive(Debug, Clone, Deserialize)]
pub struct PointAdvanceRequest {
    /// Positive advances forward, negative goes backward.
    pub value: i32,
}

/// Request body for `PUT .../course/activeRoute/pointIndex`.
#[derive(Debug, Clone, Deserialize)]
pub struct PointIndexRequest {
    /// Zero-based index of the waypoint in the route.
    pub value: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_response_roundtrip() {
        let resp = FeaturesResponse {
            apis: vec![FeatureInfo {
                id: "resources".into(),
                name: "Resources API".into(),
                enabled: true,
            }],
            plugins: vec![],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["apis"][0]["id"], "resources");
        let _: FeaturesResponse = serde_json::from_value(json).unwrap();
    }

    #[test]
    fn resource_response_roundtrip() {
        let resp = ResourceResponse {
            state: "COMPLETED".into(),
            status_code: 200,
            id: "abc-123".into(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["statusCode"], 200);
        let _: ResourceResponse = serde_json::from_value(json).unwrap();
    }

    #[test]
    fn destination_request_with_position() {
        let json = serde_json::json!({
            "position": { "latitude": 49.27, "longitude": -123.19 }
        });
        let req: DestinationRequest = serde_json::from_value(json).unwrap();
        assert!(req.position.is_some());
        assert!(req.href.is_none());
    }

    #[test]
    fn destination_request_with_href() {
        let json = serde_json::json!({
            "href": "/resources/waypoints/abc-123"
        });
        let req: DestinationRequest = serde_json::from_value(json).unwrap();
        assert!(req.position.is_none());
        assert_eq!(req.href.unwrap(), "/resources/waypoints/abc-123");
    }
}
