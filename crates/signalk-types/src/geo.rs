//! Geographic calculation utilities.
//!
//! All coordinates in degrees (latitude, longitude).
//! Bearing results in radians. Distance results in meters.

const EARTH_RADIUS: f64 = 6_371_000.0;

/// Haversine distance in meters between two lat/lon points (in degrees).
pub fn haversine_meters(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let d_lat = (lat2 - lat1).to_radians();
    let d_lon = (lon2 - lon1).to_radians();
    let a = (d_lat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    EARTH_RADIUS * c
}

/// Initial bearing (forward azimuth) in radians from point 1 to point 2.
///
/// Inputs in degrees. Result in radians [0, 2π).
pub fn initial_bearing(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let lat1 = lat1.to_radians();
    let lat2 = lat2.to_radians();
    let d_lon = (lon2 - lon1).to_radians();

    let y = d_lon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * d_lon.cos();

    y.atan2(x).rem_euclid(2.0 * std::f64::consts::PI)
}

/// Cross-track error in meters.
///
/// Signed distance from current position to the great-circle path
/// defined by `start` and `end` points.
///
/// - Negative = vessel is left of track (steer right)
/// - Positive = vessel is right of track (steer left)
///
/// All inputs as `(latitude, longitude)` in degrees.
pub fn cross_track_error(current: (f64, f64), start: (f64, f64), end: (f64, f64)) -> f64 {
    let d_start_current = haversine_meters(start.0, start.1, current.0, current.1) / EARTH_RADIUS;
    let bearing_start_current = initial_bearing(start.0, start.1, current.0, current.1);
    let bearing_start_end = initial_bearing(start.0, start.1, end.0, end.1);

    let xte = (d_start_current.sin() * (bearing_start_current - bearing_start_end).sin()).asin();

    xte * EARTH_RADIUS
}

/// Total remaining distance along a route from a given waypoint index.
///
/// Sums haversine distances between consecutive waypoints starting from `from_index`.
/// Waypoints as `(latitude, longitude)` in degrees.
pub fn route_remaining_distance(waypoints: &[(f64, f64)], from_index: usize) -> f64 {
    if from_index + 1 >= waypoints.len() {
        return 0.0;
    }
    waypoints[from_index..]
        .windows(2)
        .map(|pair| haversine_meters(pair[0].0, pair[0].1, pair[1].0, pair[1].1))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn haversine_zero_distance() {
        let d = haversine_meters(49.2744, -123.1888, 49.2744, -123.1888);
        assert!(d.abs() < 0.01);
    }

    #[test]
    fn haversine_known_distance() {
        // Vancouver to North Van: ~5.5 km
        let d = haversine_meters(49.2827, -123.1207, 49.3200, -123.0724);
        assert!((5000.0..6000.0).contains(&d), "Expected ~5.5km, got {d}m");
    }

    #[test]
    fn bearing_north() {
        // Going straight north: bearing ≈ 0
        let b = initial_bearing(49.0, -123.0, 50.0, -123.0);
        assert!(
            !(0.01..=2.0 * PI - 0.01).contains(&b),
            "Expected ~0 rad, got {b}"
        );
    }

    #[test]
    fn bearing_east() {
        // Going east from equator: bearing ≈ π/2
        let b = initial_bearing(0.0, 0.0, 0.0, 1.0);
        assert!((b - PI / 2.0).abs() < 0.01, "Expected ~π/2 rad, got {b}");
    }

    #[test]
    fn xte_on_track() {
        // Point exactly on the track should have near-zero XTE
        let xte = cross_track_error((49.15, -123.12), (49.0, -123.12), (49.3, -123.12));
        assert!(
            xte.abs() < 50.0,
            "Expected near-zero XTE on track, got {xte}m"
        );
    }

    #[test]
    fn xte_off_track() {
        // Track goes north along lon=-123, vessel is 1° east → significant XTE
        let xte = cross_track_error((49.15, -122.0), (49.0, -123.0), (49.3, -123.0));
        assert!(
            xte.abs() > 10_000.0,
            "Expected large XTE off track, got {xte}m"
        );
    }

    #[test]
    fn route_remaining_distance_simple() {
        // 3 waypoints along the same meridian, each ~111km apart
        let waypoints = vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)];
        let d = route_remaining_distance(&waypoints, 0);
        // ~222 km total
        assert!(
            (200_000.0..250_000.0).contains(&d),
            "Expected ~222km, got {d}m"
        );
    }

    #[test]
    fn route_remaining_distance_partial() {
        let waypoints = vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)];
        let d = route_remaining_distance(&waypoints, 1);
        // Only one leg remaining: ~111km
        assert!(
            (100_000.0..120_000.0).contains(&d),
            "Expected ~111km, got {d}m"
        );
    }

    #[test]
    fn route_remaining_distance_at_end() {
        let waypoints = vec![(0.0, 0.0), (1.0, 0.0)];
        let d = route_remaining_distance(&waypoints, 1);
        assert!(d.abs() < 0.01, "Expected 0 at last waypoint, got {d}");
    }
}
