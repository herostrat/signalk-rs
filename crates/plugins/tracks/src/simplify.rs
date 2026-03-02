//! Ramer-Douglas-Peucker line simplification for track segments.

use crate::types::TrackPoint;

/// Perpendicular distance from point `p` to line segment `a–b` (in degrees).
fn perpendicular_distance(p: &(f64, f64), a: &(f64, f64), b: &(f64, f64)) -> f64 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-20 {
        // a and b are the same point — use Euclidean distance to a
        let ex = p.0 - a.0;
        let ey = p.1 - a.1;
        return (ex * ex + ey * ey).sqrt();
    }
    // |cross product| / |line length|
    ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len_sq.sqrt()
}

/// Ramer-Douglas-Peucker: returns indices of points to keep.
///
/// `epsilon` is the tolerance in degrees (~0.00001° ≈ 1 m at equator).
fn rdp_indices(
    points: &[(f64, f64)],
    epsilon: f64,
    start: usize,
    end: usize,
    keep: &mut Vec<bool>,
) {
    if end <= start + 1 {
        return;
    }

    let a = &points[start];
    let b = &points[end];

    let mut max_dist = 0.0_f64;
    let mut max_idx = start;

    for (i, pt) in points.iter().enumerate().take(end).skip(start + 1) {
        let d = perpendicular_distance(&(pt.0, pt.1), a, b);
        if d > max_dist {
            max_dist = d;
            max_idx = i;
        }
    }

    if max_dist > epsilon {
        keep[max_idx] = true;
        rdp_indices(points, epsilon, start, max_idx, keep);
        rdp_indices(points, epsilon, max_idx, end, keep);
    }
}

/// Simplify a slice of `TrackPoint`s using Ramer-Douglas-Peucker.
///
/// Returns a new `Vec<TrackPoint>` with only the significant points retained.
/// Points with ≤ 2 elements or `epsilon <= 0` are returned unchanged.
pub fn simplify_track_points(points: &[TrackPoint], epsilon: f64) -> Vec<TrackPoint> {
    if points.len() <= 2 || epsilon <= 0.0 {
        return points.to_vec();
    }

    let coords: Vec<(f64, f64)> = points.iter().map(|p| (p.lat, p.lon)).collect();
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;

    rdp_indices(&coords, epsilon, 0, points.len() - 1, &mut keep);

    points
        .iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, p)| p.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn pt(lat: f64, lon: f64) -> TrackPoint {
        TrackPoint {
            lat,
            lon,
            timestamp: Utc::now(),
            sog: None,
            cog: None,
            depth: None,
        }
    }

    #[test]
    fn straight_line_simplifies_to_endpoints() {
        // 5 collinear points
        let points = vec![
            pt(0.0, 0.0),
            pt(1.0, 1.0),
            pt(2.0, 2.0),
            pt(3.0, 3.0),
            pt(4.0, 4.0),
        ];
        let result = simplify_track_points(&points, 0.001);
        assert_eq!(result.len(), 2, "collinear points → only endpoints");
        assert_eq!(result[0].lat, 0.0);
        assert_eq!(result[1].lat, 4.0);
    }

    #[test]
    fn l_shape_keeps_corner() {
        // L-shape: go east then north
        let points = vec![
            pt(0.0, 0.0),
            pt(0.0, 1.0), // corner
            pt(1.0, 1.0),
        ];
        let result = simplify_track_points(&points, 0.001);
        assert_eq!(result.len(), 3, "L-shape corner must be kept");
    }

    #[test]
    fn empty_input() {
        let result = simplify_track_points(&[], 0.001);
        assert!(result.is_empty());
    }

    #[test]
    fn single_point() {
        let result = simplify_track_points(&[pt(1.0, 2.0)], 0.001);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn two_points_unchanged() {
        let points = vec![pt(0.0, 0.0), pt(1.0, 1.0)];
        let result = simplify_track_points(&points, 0.001);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn epsilon_zero_keeps_all() {
        let points = vec![
            pt(0.0, 0.0),
            pt(0.5, 0.1), // slight deviation
            pt(1.0, 0.0),
        ];
        let result = simplify_track_points(&points, 0.0);
        assert_eq!(result.len(), 3, "epsilon=0 should keep all points");
    }

    #[test]
    fn large_epsilon_keeps_only_endpoints() {
        let points = vec![
            pt(0.0, 0.0),
            pt(0.5, 0.1),
            pt(1.0, 0.2),
            pt(1.5, 0.1),
            pt(2.0, 0.0),
        ];
        // Large epsilon: all deviations are insignificant
        let result = simplify_track_points(&points, 10.0);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn zigzag_keeps_peaks() {
        // Zigzag pattern: each peak deviates significantly
        let points = vec![
            pt(0.0, 0.0),
            pt(0.0, 1.0), // peak up
            pt(0.0, 0.0),
            pt(0.0, 1.0), // peak up
            pt(0.0, 0.0),
        ];
        let result = simplify_track_points(&points, 0.001);
        assert!(result.len() >= 3, "zigzag peaks should be kept");
    }
}
