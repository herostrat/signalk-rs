//! GPX 1.1 XML serialization for vessel tracks.
//!
//! Produces a standard GPX document with `<trk>/<trkseg>/<trkpt>`.
//! Speed, course, and depth are emitted as `<extensions>` elements.
//! No external XML crate needed — simple string concatenation.

use crate::types::VesselTrack;

/// Serialize a list of vessel tracks as GPX 1.1 XML.
pub fn tracks_to_gpx(tracks: &[VesselTrack]) -> String {
    let mut gpx = String::with_capacity(4096);
    gpx.push_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="signalk-rs" xmlns="http://www.topografix.com/GPX/1/1">
"#,
    );

    for track in tracks {
        gpx.push_str("  <trk>\n");

        let name = track.label.as_deref().unwrap_or(&track.context);
        gpx.push_str(&format!("    <name>{}</name>\n", xml_escape(name)));
        gpx.push_str(&format!(
            "    <desc>context={}</desc>\n",
            xml_escape(&track.context)
        ));

        for segment in &track.segments {
            gpx.push_str("    <trkseg>\n");
            for point in &segment.points {
                gpx.push_str(&format!(
                    "      <trkpt lat=\"{:.6}\" lon=\"{:.6}\">\n",
                    point.lat, point.lon
                ));
                gpx.push_str(&format!(
                    "        <time>{}</time>\n",
                    point.timestamp.to_rfc3339()
                ));

                // Extensions for SOG, COG, depth
                let has_ext = point.sog.is_some() || point.cog.is_some() || point.depth.is_some();
                if has_ext {
                    gpx.push_str("        <extensions>\n");
                    if let Some(sog) = point.sog {
                        gpx.push_str(&format!("          <sog>{:.2}</sog>\n", sog));
                    }
                    if let Some(cog) = point.cog {
                        gpx.push_str(&format!("          <cog>{:.4}</cog>\n", cog));
                    }
                    if let Some(depth) = point.depth {
                        gpx.push_str(&format!("          <depth>{:.1}</depth>\n", depth));
                    }
                    gpx.push_str("        </extensions>\n");
                }

                gpx.push_str("      </trkpt>\n");
            }
            gpx.push_str("    </trkseg>\n");
        }

        gpx.push_str("  </trk>\n");
    }

    gpx.push_str("</gpx>\n");
    gpx
}

/// Minimal XML escaping for text content and attribute values.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TrackPoint, TrackSegment};
    use chrono::DateTime;

    fn sample_tracks() -> Vec<VesselTrack> {
        vec![VesselTrack {
            context: "vessels.self".into(),
            label: Some("My Boat".into()),
            segments: vec![
                TrackSegment {
                    points: vec![
                        TrackPoint {
                            lat: 54.123456,
                            lon: 10.654321,
                            timestamp: "2026-02-28T08:00:00Z"
                                .parse::<DateTime<chrono::Utc>>()
                                .unwrap(),
                            sog: Some(3.15),
                            cog: Some(1.58),
                            depth: Some(12.5),
                        },
                        TrackPoint {
                            lat: 54.200000,
                            lon: 10.700000,
                            timestamp: "2026-02-28T08:05:00Z"
                                .parse::<DateTime<chrono::Utc>>()
                                .unwrap(),
                            sog: None,
                            cog: None,
                            depth: None,
                        },
                    ],
                },
                TrackSegment {
                    points: vec![TrackPoint {
                        lat: 54.500000,
                        lon: 10.500000,
                        timestamp: "2026-02-28T09:00:00Z"
                            .parse::<DateTime<chrono::Utc>>()
                            .unwrap(),
                        sog: Some(5.0),
                        cog: None,
                        depth: None,
                    }],
                },
            ],
        }]
    }

    #[test]
    fn gpx_is_valid_xml_structure() {
        let gpx = tracks_to_gpx(&sample_tracks());
        assert!(gpx.starts_with("<?xml"));
        assert!(gpx.contains("<gpx version=\"1.1\""));
        assert!(gpx.contains("</gpx>"));
        assert!(gpx.contains("<trk>"));
        assert!(gpx.contains("</trk>"));
        assert!(gpx.contains("<trkseg>"));
        assert!(gpx.contains("</trkseg>"));
    }

    #[test]
    fn gpx_contains_correct_coordinates() {
        let gpx = tracks_to_gpx(&sample_tracks());
        assert!(gpx.contains("lat=\"54.123456\" lon=\"10.654321\""));
        assert!(gpx.contains("lat=\"54.200000\" lon=\"10.700000\""));
    }

    #[test]
    fn gpx_contains_timestamps() {
        let gpx = tracks_to_gpx(&sample_tracks());
        assert!(gpx.contains("<time>2026-02-28T08:00:00"));
        assert!(gpx.contains("<time>2026-02-28T09:00:00"));
    }

    #[test]
    fn gpx_extensions_present_when_data_exists() {
        let gpx = tracks_to_gpx(&sample_tracks());
        assert!(gpx.contains("<sog>3.15</sog>"));
        assert!(gpx.contains("<cog>1.5800</cog>"));
        assert!(gpx.contains("<depth>12.5</depth>"));
    }

    #[test]
    fn gpx_no_extensions_when_no_data() {
        // Second trkpt has no sog/cog/depth — should not have <extensions>
        let gpx = tracks_to_gpx(&sample_tracks());
        // Count extensions blocks: should be 2 (first point + third point), not 3
        let ext_count = gpx.matches("<extensions>").count();
        assert_eq!(ext_count, 2);
    }

    #[test]
    fn gpx_has_two_segments() {
        let gpx = tracks_to_gpx(&sample_tracks());
        let seg_count = gpx.matches("<trkseg>").count();
        assert_eq!(seg_count, 2);
    }

    #[test]
    fn gpx_name_and_desc() {
        let gpx = tracks_to_gpx(&sample_tracks());
        assert!(gpx.contains("<name>My Boat</name>"));
        assert!(gpx.contains("<desc>context=vessels.self</desc>"));
    }

    #[test]
    fn gpx_xml_escaping() {
        let tracks = vec![VesselTrack {
            context: "vessels.self".into(),
            label: Some("Boat & <Friends>".into()),
            segments: vec![],
        }];
        let gpx = tracks_to_gpx(&tracks);
        assert!(gpx.contains("<name>Boat &amp; &lt;Friends&gt;</name>"));
    }

    #[test]
    fn gpx_empty_tracks() {
        let gpx = tracks_to_gpx(&[]);
        assert!(gpx.contains("<gpx"));
        assert!(gpx.contains("</gpx>"));
        assert!(!gpx.contains("<trk>"));
    }
}
