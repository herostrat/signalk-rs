//! REST API handlers for the tracks plugin.
//!
//! ## Spec routes (served by the server, delegating here)
//!
//! | Method | Path                                         | Description                       |
//! |--------|----------------------------------------------|-----------------------------------|
//! | GET    | `/signalk/v1/api/tracks`                     | All vessel tracks (GeoJSON/GPX)   |
//! | GET    | `/signalk/v1/api/vessels/{vessel_id}/track`   | Single vessel track               |
//! | DELETE | `/signalk/v1/api/tracks`                     | Clear all tracks                  |
//! | DELETE | `/signalk/v1/api/vessels/{vessel_id}/track`   | Clear single vessel track         |
//!
//! ## Plugin routes (`/plugins/tracks/`)
//!
//! | Method | Path       | Description                          |
//! |--------|-----------|--------------------------------------|
//! | GET    | `/summary`| Vessel summary with point counts     |

use crate::geojson::tracks_to_geojson;
use crate::gpx::tracks_to_gpx;
use crate::simplify::simplify_track_points;
use crate::store::SqliteTrackStore;
use crate::types::{TrackFormat, TrackQuery};
use chrono::DateTime;
use signalk_plugin_api::{PluginRequest, PluginResponse};
use std::collections::HashMap;
use std::sync::Arc;

/// Parse a query string into a HashMap of key-value pairs.
fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?;
            let value = parts.next().unwrap_or("");
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

/// Parse a `TrackQuery` from request query parameters.
pub fn parse_track_query(query_str: &str) -> Result<TrackQuery, String> {
    let params = parse_query(query_str);
    let mut q = TrackQuery::default();

    if let Some(ctx) = params.get("context")
        && !ctx.is_empty()
    {
        q.context = Some(ctx.clone());
    }

    if let Some(after) = params.get("after") {
        q.after = Some(
            DateTime::parse_from_rfc3339(after)
                .map_err(|e| format!("invalid 'after' timestamp: {e}"))?
                .with_timezone(&chrono::Utc),
        );
    }

    if let Some(before) = params.get("before") {
        q.before = Some(
            DateTime::parse_from_rfc3339(before)
                .map_err(|e| format!("invalid 'before' timestamp: {e}"))?
                .with_timezone(&chrono::Utc),
        );
    }

    // bbox=west,south,east,north
    if let Some(bbox_str) = params.get("bbox") {
        let parts: Vec<f64> = bbox_str
            .split(',')
            .map(|s| s.trim().parse::<f64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("invalid 'bbox': {e}"))?;
        if parts.len() != 4 {
            return Err("'bbox' must have exactly 4 values: west,south,east,north".into());
        }
        q.bbox = Some((parts[0], parts[1], parts[2], parts[3]));
    }

    // center=lat,lon&radius=meters
    if let Some(center_str) = params.get("center") {
        let parts: Vec<f64> = center_str
            .split(',')
            .map(|s| s.trim().parse::<f64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("invalid 'center': {e}"))?;
        if parts.len() != 2 {
            return Err("'center' must have exactly 2 values: lat,lon".into());
        }
        let radius: f64 = params
            .get("radius")
            .ok_or("'radius' is required when 'center' is specified")?
            .parse()
            .map_err(|e| format!("invalid 'radius': {e}"))?;
        q.radius = Some((parts[0], parts[1], radius));
    }

    if let Some(limit_str) = params.get("limit") {
        q.limit = Some(
            limit_str
                .parse()
                .map_err(|e| format!("invalid 'limit': {e}"))?,
        );
    }

    if let Some(s) = params.get("simplify") {
        q.simplify = Some(s.parse().map_err(|e| format!("invalid 'simplify': {e}"))?);
    }

    if let Some(fmt) = params.get("format") {
        q.format = match fmt.as_str() {
            "gpx" => TrackFormat::Gpx,
            "geojson" => TrackFormat::GeoJson,
            other => {
                return Err(format!(
                    "unknown format '{other}', expected 'geojson' or 'gpx'"
                ));
            }
        };
    }

    Ok(q)
}

/// Handle `GET /` — query tracks and return GeoJSON or GPX.
pub fn handle_get_tracks(store: &Arc<SqliteTrackStore>, req: &PluginRequest) -> PluginResponse {
    let query = match parse_track_query(req.query.as_deref().unwrap_or("")) {
        Ok(q) => q,
        Err(e) => return PluginResponse::json(400, &serde_json::json!({ "error": e })),
    };

    let format = query.format;
    let simplify_epsilon = query.simplify;
    let mut tracks = store.query(&query);

    // Apply Douglas-Peucker simplification if requested
    if let Some(epsilon) = simplify_epsilon {
        for track in &mut tracks {
            for seg in &mut track.segments {
                seg.points = simplify_track_points(&seg.points, epsilon);
            }
        }
    }

    match format {
        TrackFormat::GeoJson => {
            let geojson = tracks_to_geojson(&tracks);
            PluginResponse::json(200, &geojson)
        }
        TrackFormat::Gpx => {
            let gpx = tracks_to_gpx(&tracks);
            PluginResponse {
                status: 200,
                headers: vec![
                    ("Content-Type".into(), "application/gpx+xml".into()),
                    (
                        "Content-Disposition".into(),
                        "attachment; filename=\"track.gpx\"".into(),
                    ),
                ],
                body: gpx.into_bytes(),
            }
        }
    }
}

/// Handle `GET /summary` — vessel track summary.
pub fn handle_get_summary(store: &Arc<SqliteTrackStore>) -> PluginResponse {
    let summary = store.summary();
    PluginResponse::json(200, &summary)
}

/// Handle `DELETE /` — clear tracks for one or all vessels.
pub fn handle_delete_tracks(store: &Arc<SqliteTrackStore>, req: &PluginRequest) -> PluginResponse {
    let params = parse_query(req.query.as_deref().unwrap_or(""));

    if let Some(context) = params.get("context") {
        store.clear_vessel(context);
        PluginResponse::json(
            200,
            &serde_json::json!({ "message": format!("cleared tracks for {context}") }),
        )
    } else {
        store.clear_all();
        PluginResponse::json(200, &serde_json::json!({ "message": "all tracks cleared" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrackPoint;
    use chrono::Utc;
    use std::sync::Mutex;

    fn make_store() -> Arc<SqliteTrackStore> {
        let db = signalk_sqlite::Database::open_in_memory().unwrap();
        let store = SqliteTrackStore::new(Arc::new(Mutex::new(db.into_conn())));
        store.record(
            "vessels.self",
            TrackPoint {
                lat: 54.0,
                lon: 10.0,
                timestamp: Utc::now(),
                sog: None,
                cog: None,
                depth: None,
            },
        );
        Arc::new(store)
    }

    #[test]
    fn parse_query_defaults() {
        let q = parse_track_query("").unwrap();
        assert!(q.context.is_none());
        assert!(q.after.is_none());
        assert!(q.bbox.is_none());
        assert!(q.radius.is_none());
        assert!(q.limit.is_none());
        assert_eq!(q.format, TrackFormat::GeoJson);
    }

    #[test]
    fn parse_query_all_params() {
        let qs = "context=vessels.self&after=2026-02-28T00:00:00Z&before=2026-02-28T23:59:59Z&bbox=9.0,53.0,11.0,55.0&limit=500&format=gpx";
        let q = parse_track_query(qs).unwrap();
        assert_eq!(q.context.as_deref(), Some("vessels.self"));
        assert!(q.after.is_some());
        assert!(q.before.is_some());
        assert_eq!(q.bbox, Some((9.0, 53.0, 11.0, 55.0)));
        assert_eq!(q.limit, Some(500));
        assert_eq!(q.format, TrackFormat::Gpx);
    }

    #[test]
    fn parse_query_radius() {
        let qs = "center=54.0,10.0&radius=5000";
        let q = parse_track_query(qs).unwrap();
        assert_eq!(q.radius, Some((54.0, 10.0, 5000.0)));
    }

    #[test]
    fn parse_query_radius_missing_radius() {
        let qs = "center=54.0,10.0";
        assert!(parse_track_query(qs).is_err());
    }

    #[test]
    fn parse_query_invalid_bbox() {
        let qs = "bbox=1.0,2.0,3.0";
        assert!(parse_track_query(qs).is_err());
    }

    #[test]
    fn parse_query_invalid_format() {
        let qs = "format=csv";
        assert!(parse_track_query(qs).is_err());
    }

    #[test]
    fn get_tracks_returns_geojson() {
        let store = make_store();
        let req = PluginRequest {
            method: "GET".into(),
            path: "/".into(),
            query: None,
            headers: vec![],
            body: vec![],
        };

        let resp = handle_get_tracks(&store, &req);
        assert_eq!(resp.status, 200);

        let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(body["type"], "FeatureCollection");
        assert_eq!(body["features"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn get_tracks_gpx_format() {
        let store = make_store();
        let req = PluginRequest {
            method: "GET".into(),
            path: "/".into(),
            query: Some("format=gpx".into()),
            headers: vec![],
            body: vec![],
        };

        let resp = handle_get_tracks(&store, &req);
        assert_eq!(resp.status, 200);
        assert!(
            resp.headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v == "application/gpx+xml")
        );

        let body = String::from_utf8(resp.body).unwrap();
        assert!(body.contains("<gpx"));
    }

    #[test]
    fn get_summary() {
        let store = make_store();
        let resp = handle_get_summary(&store);
        assert_eq!(resp.status, 200);

        let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["context"], "vessels.self");
        assert_eq!(arr[0]["point_count"], 1);
    }

    #[test]
    fn delete_all_tracks() {
        let store = make_store();
        let req = PluginRequest {
            method: "DELETE".into(),
            path: "/".into(),
            query: None,
            headers: vec![],
            body: vec![],
        };

        let resp = handle_delete_tracks(&store, &req);
        assert_eq!(resp.status, 200);
        assert_eq!(store.total_points(), 0);
    }

    #[test]
    fn delete_single_vessel() {
        let store = make_store();
        // Add another vessel
        store.record(
            "vessels.other",
            TrackPoint {
                lat: 55.0,
                lon: 11.0,
                timestamp: Utc::now(),
                sog: None,
                cog: None,
                depth: None,
            },
        );

        let req = PluginRequest {
            method: "DELETE".into(),
            path: "/".into(),
            query: Some("context=vessels.self".into()),
            headers: vec![],
            body: vec![],
        };

        let resp = handle_delete_tracks(&store, &req);
        assert_eq!(resp.status, 200);
        assert_eq!(store.vessel_count(), 1);
        assert_eq!(store.total_points(), 1);
    }

    #[test]
    fn invalid_query_returns_400() {
        let store = make_store();
        let req = PluginRequest {
            method: "GET".into(),
            path: "/".into(),
            query: Some("after=not-a-date".into()),
            headers: vec![],
            body: vec![],
        };

        let resp = handle_get_tracks(&store, &req);
        assert_eq!(resp.status, 400);
    }
}
