//! Closest Point of Approach (CPA) and Time to CPA (TCPA) calculation.
//!
//! Uses equirectangular projection — accurate to within ~1% for distances
//! up to ~50 nautical miles.  All inputs and outputs use SI units (meters,
//! seconds, radians).
//!
//! # Algorithm
//!
//! ```text
//! P_rel = target_pos − own_pos      (metres, equirectangular)
//! V_rel = target_vel − own_vel      (m/s, derived from SOG/COG)
//!
//! TCPA = −dot(P_rel, V_rel) / |V_rel|²
//! CPA  = |P_rel + V_rel × TCPA|
//! ```
//!
//! TCPA < 0 means closest approach was in the past; the threat is receding.
//! |V_rel| ≈ 0 (same velocity vectors) → CPA = current distance, TCPA = +∞.

const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Result of a CPA calculation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpaResult {
    /// Closest Point of Approach distance in metres.
    pub cpa_m: f64,
    /// Time to CPA in seconds.  Negative = CPA already passed.
    /// `+∞` when both vessels have the same velocity vector.
    pub tcpa_s: f64,
}

/// Compute CPA/TCPA between own vessel and a target.
///
/// # Parameters
/// - `own_lat`, `own_lon`   : own position in decimal degrees
/// - `own_sog_ms`           : own speed over ground in m/s
/// - `own_cog_rad`          : own course over ground in radians (0 = north, clockwise)
/// - `tgt_lat`, `tgt_lon`  : target position in decimal degrees
/// - `tgt_sog_ms`           : target speed over ground in m/s
/// - `tgt_cog_rad`          : target course over ground in radians
///
/// Returns `None` if any input value is non-finite (NaN or ±infinity).
#[allow(clippy::too_many_arguments)]
pub fn compute_cpa(
    own_lat: f64,
    own_lon: f64,
    own_sog_ms: f64,
    own_cog_rad: f64,
    tgt_lat: f64,
    tgt_lon: f64,
    tgt_sog_ms: f64,
    tgt_cog_rad: f64,
) -> Option<CpaResult> {
    // Reject non-finite inputs
    if ![
        own_lat,
        own_lon,
        own_sog_ms,
        own_cog_rad,
        tgt_lat,
        tgt_lon,
        tgt_sog_ms,
        tgt_cog_rad,
    ]
    .iter()
    .all(|v| v.is_finite())
    {
        return None;
    }

    // Equirectangular projection around the mid-latitude
    let mid_lat = ((own_lat + tgt_lat) / 2.0).to_radians();
    let cos_mid = mid_lat.cos();

    // Relative position vector (target − own), in metres
    let px = (tgt_lon - own_lon).to_radians() * cos_mid * EARTH_RADIUS_M;
    let py = (tgt_lat - own_lat).to_radians() * EARTH_RADIUS_M;

    // Velocity components (east, north) in m/s
    let (own_vx, own_vy) = sog_cog_to_xy(own_sog_ms, own_cog_rad);
    let (tgt_vx, tgt_vy) = sog_cog_to_xy(tgt_sog_ms, tgt_cog_rad);

    // Relative velocity vector (target − own)
    let vx = tgt_vx - own_vx;
    let vy = tgt_vy - own_vy;

    let v_sq = vx * vx + vy * vy;

    let tcpa_s = if v_sq < 1e-6 {
        // Virtually identical velocity vectors — distance stays constant
        f64::INFINITY
    } else {
        -(px * vx + py * vy) / v_sq
    };

    let cpa_m = if tcpa_s.is_infinite() {
        // Both stationary or identical velocity: CPA = current distance
        (px * px + py * py).sqrt()
    } else if tcpa_s < 0.0 {
        // Closest approach already happened; CPA = current distance at t=TCPA
        // (distance at closest point in the past — return as positive magnitude)
        let cx = px + vx * tcpa_s;
        let cy = py + vy * tcpa_s;
        (cx * cx + cy * cy).sqrt()
    } else {
        let cx = px + vx * tcpa_s;
        let cy = py + vy * tcpa_s;
        (cx * cx + cy * cy).sqrt()
    };

    Some(CpaResult { cpa_m, tcpa_s })
}

/// Convert SOG (m/s) + COG (radians, 0=north clockwise) to (east, north) m/s.
#[inline]
fn sog_cog_to_xy(sog: f64, cog: f64) -> (f64, f64) {
    (sog * cog.sin(), sog * cog.cos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Head-on collision: own vessel heading north, target heading south from 1 NM ahead.
    /// Both at 5 m/s. They will meet in the middle (~500 m ahead) in ~185 s.
    #[test]
    fn head_on_collision() {
        // Own: 54°N 10°E, heading north (0 rad) at 5 m/s
        // Target: 54.009°N 10°E (~1 km ahead), heading south (π rad) at 5 m/s
        let result = compute_cpa(54.0, 10.0, 5.0, 0.0, 54.009, 10.0, 5.0, PI).unwrap();

        // CPA should be very small (near-zero); TCPA should be positive
        assert!(
            result.cpa_m < 50.0,
            "CPA should be near 0, got {}",
            result.cpa_m
        );
        assert!(
            result.tcpa_s > 0.0,
            "TCPA should be positive (future), got {}",
            result.tcpa_s
        );
        assert!(result.tcpa_s < 600.0, "TCPA too large: {}", result.tcpa_s);
    }

    /// Parallel courses: same direction, laterally separated.
    /// CPA should equal the lateral separation; TCPA = infinity.
    #[test]
    fn parallel_courses_same_direction() {
        // Own: 54°N 10°E heading north at 5 m/s
        // Target: 54°N 10.01°E (same latitude, ~700 m east) heading north at 5 m/s
        let result = compute_cpa(54.0, 10.0, 5.0, 0.0, 54.0, 10.01, 5.0, 0.0).unwrap();

        // Same velocity → TCPA = +infinity, CPA = current lateral distance
        assert!(
            result.tcpa_s.is_infinite(),
            "TCPA should be +∞ for same velocity"
        );
        // ~700 m lateral separation at 54° lat
        assert!(
            result.cpa_m > 500.0 && result.cpa_m < 900.0,
            "CPA should be ~700 m, got {}",
            result.cpa_m
        );
    }

    /// Diverging: past CPA. Both heading east, target is ahead and faster.
    /// The target already passed own vessel — closest approach was in the past.
    #[test]
    fn diverging_past_cpa() {
        // Own: 54°N 10°E heading east (π/2) at 5 m/s
        // Target: 54°N 10.004°E (≈260m ahead) also heading east at 8 m/s
        // V_rel = (3, 0) m/s — target accelerating away; TCPA < 0.
        let result = compute_cpa(54.0, 10.0, 5.0, PI / 2.0, 54.0, 10.004, 8.0, PI / 2.0).unwrap();

        // TCPA < 0 means closest approach is already in the past
        assert!(
            result.tcpa_s < 0.0,
            "TCPA should be negative (past), got {}",
            result.tcpa_s
        );
    }

    /// Stationary target: CPA = current distance, TCPA = +infinity.
    #[test]
    fn stationary_target() {
        // Own: 54°N 10°E heading north at 5 m/s
        // Target: 54.005°N 10°E (~555 m ahead), stationary
        let result = compute_cpa(54.0, 10.0, 5.0, 0.0, 54.005, 10.0, 0.0, 0.0).unwrap();

        // Target is stationary but own vessel is moving → should have positive TCPA
        // (own vessel approaching stationary target)
        assert!(
            result.tcpa_s > 0.0,
            "TCPA should be positive, got {}",
            result.tcpa_s
        );
        // CPA should be near 0 (will hit stationary target)
        assert!(
            result.cpa_m < 50.0,
            "CPA should be near 0, got {}",
            result.cpa_m
        );
    }

    /// Non-finite inputs return None.
    #[test]
    fn non_finite_input_returns_none() {
        assert!(compute_cpa(f64::NAN, 10.0, 5.0, 0.0, 54.0, 10.0, 5.0, 0.0).is_none());
        assert!(compute_cpa(54.0, 10.0, f64::INFINITY, 0.0, 54.0, 10.0, 5.0, 0.0).is_none());
        assert!(compute_cpa(54.0, 10.0, 5.0, f64::NAN, 54.0, 10.0, 5.0, 0.0).is_none());
        assert!(compute_cpa(54.0, 10.0, 5.0, 0.0, f64::NAN, 10.0, 5.0, 0.0).is_none());
    }

    /// Both stationary: CPA = current distance, TCPA = +infinity.
    #[test]
    fn both_stationary() {
        // 1 NM separation (due north)
        let result = compute_cpa(54.0, 10.0, 0.0, 0.0, 54.009, 10.0, 0.0, 0.0).unwrap();

        assert!(
            result.tcpa_s.is_infinite(),
            "TCPA should be +∞ when both stationary"
        );
        // ~1 km distance
        assert!(
            result.cpa_m > 500.0 && result.cpa_m < 1500.0,
            "CPA should be ~1 km, got {}",
            result.cpa_m
        );
    }

    /// Crossing: own heading east, target heading west from 1 NM ahead.
    /// They should be on a collision course (very small CPA) near the midpoint.
    #[test]
    fn crossing_courses() {
        // Own: 54°N 10°E heading east (π/2) at 5 m/s
        // Target: 54°N 10.018°E (≈1 NM east) heading west (3π/2) at 5 m/s
        let result =
            compute_cpa(54.0, 10.0, 5.0, PI / 2.0, 54.0, 10.018, 5.0, 3.0 * PI / 2.0).unwrap();

        assert!(result.tcpa_s > 0.0, "TCPA should be positive");
        assert!(
            result.cpa_m < 100.0,
            "Should be near-collision, CPA={}",
            result.cpa_m
        );
    }

    // ── Edge cases ──────────────────────────────────────────────────

    /// Own vessel anchored (SOG=0), target approaching. CPA should be near-zero,
    /// TCPA positive.
    #[test]
    fn own_vessel_stationary() {
        // Own: 54°N 10°E, anchored (SOG=0)
        // Target: 54.009°N 10°E (~1km north), heading south at 5 m/s
        let result = compute_cpa(54.0, 10.0, 0.0, 0.0, 54.009, 10.0, 5.0, PI).unwrap();
        assert!(
            result.tcpa_s > 0.0,
            "TCPA should be positive: target approaching"
        );
        assert!(
            result.cpa_m < 50.0,
            "CPA near-zero: target heading directly for us"
        );
    }

    /// CPA distance must always be >= 0 for any valid inputs.
    #[test]
    fn cpa_result_is_always_non_negative() {
        let cases = [
            (
                54.0_f64, 10.0_f64, 5.0_f64, 0.0_f64, 54.009_f64, 10.0_f64, 5.0_f64, PI,
            ),
            (54.0, 10.0, 5.0, 0.0, 54.0, 10.01, 5.0, 0.0),
            (54.0, 10.0, 5.0, PI / 2.0, 54.0, 10.004, 8.0, PI / 2.0),
            (54.0, 10.0, 0.0, 0.0, 54.009, 10.0, 5.0, PI), // own anchored
        ];
        for (own_lat, own_lon, own_sog, own_cog, tgt_lat, tgt_lon, tgt_sog, tgt_cog) in cases {
            let result = compute_cpa(
                own_lat, own_lon, own_sog, own_cog, tgt_lat, tgt_lon, tgt_sog, tgt_cog,
            )
            .unwrap();
            assert!(
                result.cpa_m >= 0.0,
                "CPA must be >= 0, got {} (tcpa={})",
                result.cpa_m,
                result.tcpa_s
            );
        }
    }

    /// COG = 0 (north): velocity vector is (vx=0, vy=SOG).
    /// Verify via a head-on scenario: own north, target south from ahead → collision.
    #[test]
    fn due_north_heading_produces_correct_vector() {
        // Own heading north (COG=0), target approaching from north heading south
        let result = compute_cpa(54.0, 10.0, 5.0, 0.0, 54.009, 10.0, 5.0, PI).unwrap();
        // Head-on → very small CPA, positive TCPA
        assert!(result.cpa_m < 50.0);
        assert!(result.tcpa_s > 0.0);
    }

    /// COG = π/2 (east): velocity vector is (vx=SOG, vy=0).
    /// Two vessels heading east in parallel (same heading, same SOG, laterally offset)
    /// → same velocity vector → TCPA = +∞.
    #[test]
    fn due_east_heading_produces_correct_vector() {
        // Own: 54°N 10°E heading east at 5 m/s
        // Target: 54.001°N 10°E (~111m north), also heading east at 5 m/s
        let result = compute_cpa(54.0, 10.0, 5.0, PI / 2.0, 54.001, 10.0, 5.0, PI / 2.0).unwrap();
        assert!(
            result.tcpa_s.is_infinite(),
            "Parallel same-velocity vessels: TCPA = +∞, got {}",
            result.tcpa_s
        );
        // CPA = lateral separation ~111m
        assert!(result.cpa_m > 50.0 && result.cpa_m < 200.0);
    }

    /// Very large distance (100 km): computation should not panic or produce NaN.
    #[test]
    fn large_distance_does_not_panic() {
        // Own: equator; target: ~100km north
        let result = compute_cpa(0.0, 0.0, 5.0, 0.0, 0.9, 0.0, 5.0, PI);
        assert!(result.is_some(), "Should return Some for large distance");
        let r = result.unwrap();
        assert!(r.cpa_m.is_finite(), "CPA should be finite");
        assert!(r.cpa_m >= 0.0);
    }

    /// Same position, same velocity: relative position is zero, v_sq < threshold → TCPA = +∞, CPA = 0.
    #[test]
    fn same_position_and_velocity() {
        let result = compute_cpa(54.0, 10.0, 5.0, 0.0, 54.0, 10.0, 5.0, 0.0).unwrap();
        assert!(result.tcpa_s.is_infinite());
        assert!(result.cpa_m < 1.0, "CPA should be ~0 when at same position");
    }
}
