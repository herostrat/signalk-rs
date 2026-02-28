//! GeoJSON serialization for vessel tracks.
//!
//! Produces a `FeatureCollection` where each `Feature` represents one vessel.
//! The geometry is a `MultiLineString` (one LineString per segment).
//! GeoJSON coordinates are `[longitude, latitude]` per RFC 7946.

use crate::types::VesselTrack;

/// Serialize a list of vessel tracks as a GeoJSON `FeatureCollection`.
pub fn tracks_to_geojson(tracks: &[VesselTrack]) -> serde_json::Value {
    let features: Vec<serde_json::Value> = tracks.iter().map(track_to_feature).collect();

    serde_json::json!({
        "type": "FeatureCollection",
        "features": features
    })
}

/// Convert a single vessel track into a GeoJSON Feature with MultiLineString geometry.
fn track_to_feature(track: &VesselTrack) -> serde_json::Value {
    let total_points: usize = track.segments.iter().map(|s| s.points.len()).sum();

    let oldest = track
        .segments
        .first()
        .and_then(|s| s.points.first())
        .map(|p| p.timestamp.to_rfc3339());

    let newest = track
        .segments
        .last()
        .and_then(|s| s.points.last())
        .map(|p| p.timestamp.to_rfc3339());

    let coordinates: Vec<Vec<Vec<f64>>> = track
        .segments
        .iter()
        .map(|seg| {
            seg.points
                .iter()
                .map(|p| vec![p.lon, p.lat]) // GeoJSON: [lon, lat]
                .collect()
        })
        .collect();

    let mut properties = serde_json::json!({
        "context": track.context,
        "pointCount": total_points,
    });

    if let Some(ref label) = track.label {
        properties["label"] = serde_json::json!(label);
    }
    if let Some(ref ts) = oldest {
        properties["oldest"] = serde_json::json!(ts);
    }
    if let Some(ref ts) = newest {
        properties["newest"] = serde_json::json!(ts);
    }

    serde_json::json!({
        "type": "Feature",
        "properties": properties,
        "geometry": {
            "type": "MultiLineString",
            "coordinates": coordinates
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TrackPoint, TrackSegment};
    use chrono::{DateTime, Utc};

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn sample_track() -> VesselTrack {
        VesselTrack {
            context: "vessels.self".into(),
            label: Some("My Boat".into()),
            segments: vec![
                TrackSegment {
                    points: vec![
                        TrackPoint {
                            lat: 54.0,
                            lon: 10.0,
                            timestamp: ts("2026-02-28T08:00:00Z"),
                            sog: Some(3.0),
                            cog: Some(1.57),
                            depth: Some(12.0),
                        },
                        TrackPoint {
                            lat: 54.1,
                            lon: 10.1,
                            timestamp: ts("2026-02-28T08:05:00Z"),
                            sog: None,
                            cog: None,
                            depth: None,
                        },
                    ],
                },
                TrackSegment {
                    points: vec![TrackPoint {
                        lat: 54.5,
                        lon: 10.5,
                        timestamp: ts("2026-02-28T09:00:00Z"),
                        sog: Some(5.0),
                        cog: None,
                        depth: None,
                    }],
                },
            ],
        }
    }

    #[test]
    fn feature_collection_structure() {
        let tracks = vec![sample_track()];
        let geojson = tracks_to_geojson(&tracks);

        assert_eq!(geojson["type"], "FeatureCollection");
        let features = geojson["features"].as_array().unwrap();
        assert_eq!(features.len(), 1);
        assert_eq!(features[0]["type"], "Feature");
    }

    #[test]
    fn coordinates_are_lon_lat() {
        let tracks = vec![sample_track()];
        let geojson = tracks_to_geojson(&tracks);

        let geometry = &geojson["features"][0]["geometry"];
        assert_eq!(geometry["type"], "MultiLineString");

        let coords = geometry["coordinates"].as_array().unwrap();
        // Two segments
        assert_eq!(coords.len(), 2);

        // First segment, first point: [lon=10.0, lat=54.0]
        let first_point = &coords[0][0];
        assert_eq!(first_point[0].as_f64().unwrap(), 10.0);
        assert_eq!(first_point[1].as_f64().unwrap(), 54.0);
    }

    #[test]
    fn properties_include_metadata() {
        let tracks = vec![sample_track()];
        let geojson = tracks_to_geojson(&tracks);

        let props = &geojson["features"][0]["properties"];
        assert_eq!(props["context"], "vessels.self");
        assert_eq!(props["label"], "My Boat");
        assert_eq!(props["pointCount"], 3);
        assert!(props["oldest"].as_str().unwrap().contains("08:00:00"));
        assert!(props["newest"].as_str().unwrap().contains("09:00:00"));
    }

    #[test]
    fn multi_segment_produces_multiple_linestrings() {
        let tracks = vec![sample_track()];
        let geojson = tracks_to_geojson(&tracks);

        let coords = geojson["features"][0]["geometry"]["coordinates"]
            .as_array()
            .unwrap();
        assert_eq!(coords.len(), 2);
        assert_eq!(coords[0].as_array().unwrap().len(), 2); // 2 points
        assert_eq!(coords[1].as_array().unwrap().len(), 1); // 1 point
    }

    #[test]
    fn empty_tracks_produce_empty_collection() {
        let geojson = tracks_to_geojson(&[]);
        assert_eq!(geojson["type"], "FeatureCollection");
        assert!(geojson["features"].as_array().unwrap().is_empty());
    }

    #[test]
    fn multiple_vessels() {
        let t1 = sample_track();
        let t2 = VesselTrack {
            context: "vessels.urn:mrn:imo:mmsi:211000001".into(),
            label: None,
            segments: vec![TrackSegment {
                points: vec![TrackPoint {
                    lat: 53.0,
                    lon: 9.0,
                    timestamp: ts("2026-02-28T10:00:00Z"),
                    sog: None,
                    cog: None,
                    depth: None,
                }],
            }],
        };

        let geojson = tracks_to_geojson(&[t1, t2]);
        let features = geojson["features"].as_array().unwrap();
        assert_eq!(features.len(), 2);
        // Second vessel has no label → field absent
        assert!(features[1]["properties"].get("label").is_none());
    }
}
