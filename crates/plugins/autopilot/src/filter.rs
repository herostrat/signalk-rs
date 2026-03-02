//! Low-pass filters for autopilot sensor smoothing.
//!
//! - [`LowPassFilter`]: First-order EMA for non-circular signals (speed, etc.)
//! - [`CircularFilter`]: Wrap-safe filter for angular signals (heading, wind angle)
//!
//! # Choosing alpha
//!
//! Given a sampling interval `dt` (seconds) and desired time constant `τ` (seconds):
//!
//! ```text
//!   α = dt / (τ + dt)
//! ```
//!
//! Examples (at 10 Hz, dt = 0.1):
//! - τ = 0.5 s → α ≈ 0.17  (light smoothing)
//! - τ = 2 s   → α ≈ 0.05  (moderate smoothing, good for wind angle)
//! - τ = 10 s  → α ≈ 0.01  (heavy smoothing, good for very noisy signals)
//!
//! Use `alpha = 1.0` for passthrough (no filtering).

// ─── Linear low-pass filter ──────────────────────────────────────────────────

/// First-order low-pass filter (exponential moving average).
///
/// ```text
///   y[n] = y[n-1] + α * (x[n] - y[n-1])
/// ```
///
/// Suitable for non-circular signals (speed, distance, etc.).
/// For angular signals, use [`CircularFilter`] instead.
pub struct LowPassFilter {
    value: Option<f64>,
    alpha: f64,
}

impl LowPassFilter {
    /// Create a new filter with the given smoothing factor.
    ///
    /// # Panics
    /// Panics if `alpha` is not in the range `(0, 1]`.
    pub fn new(alpha: f64) -> Self {
        assert!(
            alpha > 0.0 && alpha <= 1.0,
            "alpha must be in (0, 1], got {alpha}"
        );
        LowPassFilter { value: None, alpha }
    }

    /// Apply one sample. Returns the filtered output.
    ///
    /// The first sample initialises the filter (no lag on startup).
    pub fn update(&mut self, input: f64) -> f64 {
        let out = match self.value {
            Some(prev) => prev + self.alpha * (input - prev),
            None => input,
        };
        self.value = Some(out);
        out
    }

    /// Reset the filter state (next update will initialise again).
    pub fn reset(&mut self) {
        self.value = None;
    }

    /// Return the current filtered value, or `None` if not yet initialised.
    pub fn get(&self) -> Option<f64> {
        self.value
    }
}

// ─── Circular (angular) low-pass filter ──────────────────────────────────────

/// Wrap-safe low-pass filter for angular signals.
///
/// Filters sin(θ) and cos(θ) components separately, then reconstructs the
/// angle with `atan2`. This avoids the glitch at the ±π boundary where a
/// linear filter would interpolate through 0° instead of through ±180°.
///
/// # Example
/// ```text
///   Input: 3.10, 3.14, -3.14, -3.10   (crossing dead upwind)
///   Linear filter: output dips toward 0.0 (wrong!)
///   CircularFilter: output stays near ±π (correct)
/// ```
pub struct CircularFilter {
    sin_filter: LowPassFilter,
    cos_filter: LowPassFilter,
}

impl CircularFilter {
    /// Create a new circular filter with the given smoothing factor.
    ///
    /// # Panics
    /// Panics if `alpha` is not in the range `(0, 1]`.
    pub fn new(alpha: f64) -> Self {
        CircularFilter {
            sin_filter: LowPassFilter::new(alpha),
            cos_filter: LowPassFilter::new(alpha),
        }
    }

    /// Apply one angular sample (radians). Returns the filtered angle in `[-π, π]`.
    pub fn update(&mut self, angle_rad: f64) -> f64 {
        let s = self.sin_filter.update(angle_rad.sin());
        let c = self.cos_filter.update(angle_rad.cos());
        s.atan2(c)
    }

    /// Reset the filter state (next update will initialise again).
    pub fn reset(&mut self) {
        self.sin_filter.reset();
        self.cos_filter.reset();
    }

    /// Return the current filtered angle, or `None` if not yet initialised.
    pub fn get(&self) -> Option<f64> {
        match (self.sin_filter.get(), self.cos_filter.get()) {
            (Some(s), Some(c)) => Some(s.atan2(c)),
            _ => None,
        }
    }
}

// ─── Rate-of-change detector ────────────────────────────────────────────────

/// Tracks the rate of change (derivative) of a signal.
///
/// Uses a first-order difference (`(x[n] - x[n-1]) / dt`) with optional
/// low-pass smoothing on the output to reduce noise.
///
/// Primary use: gust detection — computes d(AWS)/dt from wind speed samples.
pub struct RateDetector {
    prev_value: Option<f64>,
    output_filter: LowPassFilter,
}

impl RateDetector {
    /// Create a new rate detector.
    ///
    /// `smoothing_alpha` controls noise rejection on the rate output.
    /// Use 1.0 for raw rate, 0.3–0.5 for moderate smoothing.
    pub fn new(smoothing_alpha: f64) -> Self {
        RateDetector {
            prev_value: None,
            output_filter: LowPassFilter::new(smoothing_alpha),
        }
    }

    /// Feed a new sample and return the smoothed rate of change.
    ///
    /// Returns 0.0 for the first sample (no previous value to diff against).
    pub fn update(&mut self, value: f64, dt: f64) -> f64 {
        let rate = match self.prev_value {
            Some(prev) if dt > 0.0 => (value - prev) / dt,
            _ => 0.0,
        };
        self.prev_value = Some(value);
        self.output_filter.update(rate)
    }

    /// Reset the detector (next update returns 0.0).
    pub fn reset(&mut self) {
        self.prev_value = None;
        self.output_filter.reset();
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::f64::consts::PI;

    // ── LowPassFilter ───────────────────────────────────────────────────────

    #[test]
    fn lpf_initializes_to_first_sample() {
        let mut f = LowPassFilter::new(0.5);
        assert_eq!(f.update(3.0), 3.0);
    }

    #[test]
    fn lpf_converges_toward_input() {
        let mut f = LowPassFilter::new(0.5);
        f.update(0.0);
        let out = f.update(10.0);
        assert!(out > 0.0 && out < 10.0);
    }

    #[test]
    fn lpf_passthrough_with_alpha_one() {
        let mut f = LowPassFilter::new(1.0);
        f.update(1.0);
        assert_eq!(f.update(42.0), 42.0);
    }

    #[test]
    fn lpf_output_stays_between_last_and_input() {
        let mut f = LowPassFilter::new(0.3);
        f.update(5.0);
        let out = f.update(10.0);
        assert!((5.0..=10.0).contains(&out));
    }

    #[test]
    fn lpf_reset_reinitializes() {
        let mut f = LowPassFilter::new(0.5);
        f.update(10.0);
        f.reset();
        assert_eq!(f.update(5.0), 5.0);
    }

    #[test]
    fn lpf_get_none_before_update() {
        let f = LowPassFilter::new(0.5);
        assert!(f.get().is_none());
    }

    #[test]
    fn lpf_get_returns_value_after_update() {
        let mut f = LowPassFilter::new(0.5);
        f.update(7.0);
        assert_eq!(f.get(), Some(7.0));
    }

    #[test]
    fn lpf_heavy_smoothing_changes_slowly() {
        let mut f = LowPassFilter::new(0.05);
        f.update(0.0);
        let out = f.update(100.0);
        assert!(out < 10.0);
    }

    // ── CircularFilter ──────────────────────────────────────────────────────

    #[test]
    fn circular_initializes_to_first_sample() {
        let mut f = CircularFilter::new(0.5);
        let out = f.update(1.0);
        assert!((out - 1.0).abs() < 1e-10);
    }

    #[test]
    fn circular_stays_near_pi_when_crossing_boundary() {
        // The key test: crossing ±π should NOT dip toward 0
        let mut f = CircularFilter::new(0.3);
        f.update(3.1); // just below π
        f.update(std::f64::consts::PI);
        let out1 = f.update(-std::f64::consts::PI); // just past −π
        let out2 = f.update(-3.1);

        // Both outputs should be near ±π, NOT near 0
        assert!(
            out1.abs() > 2.5,
            "should stay near ±π when crossing boundary, got {out1}"
        );
        assert!(
            out2.abs() > 2.5,
            "should stay near ±π when crossing boundary, got {out2}"
        );
    }

    #[test]
    fn circular_converges_away_from_boundary() {
        // Away from the boundary, behaves like a normal filter
        let mut f = CircularFilter::new(0.5);
        f.update(0.0);
        let out = f.update(1.0);
        assert!(out > 0.0 && out < 1.0);
    }

    #[test]
    fn circular_reset_reinitializes() {
        let mut f = CircularFilter::new(0.5);
        f.update(1.0);
        f.reset();
        let out = f.update(2.0);
        assert!((out - 2.0).abs() < 1e-10);
    }

    #[test]
    fn circular_get_none_before_update() {
        let f = CircularFilter::new(0.5);
        assert!(f.get().is_none());
    }

    #[test]
    fn circular_get_returns_value() {
        let mut f = CircularFilter::new(0.5);
        f.update(1.5);
        assert!(f.get().is_some());
        assert!((f.get().unwrap() - 1.5).abs() < 1e-10);
    }

    // ── RateDetector ──────────────────────────────────────────────────────

    #[test]
    fn rate_detector_first_sample_returns_zero() {
        let mut r = RateDetector::new(1.0);
        assert_eq!(r.update(10.0, 0.1), 0.0);
    }

    #[test]
    fn rate_detector_constant_signal_returns_zero() {
        let mut r = RateDetector::new(1.0);
        r.update(5.0, 0.1);
        let rate = r.update(5.0, 0.1);
        assert!(rate.abs() < 1e-10);
    }

    #[test]
    fn rate_detector_positive_ramp() {
        let mut r = RateDetector::new(1.0); // no smoothing
        r.update(0.0, 0.1);
        let rate = r.update(1.0, 0.1);
        // d/dt = (1.0 - 0.0) / 0.1 = 10.0
        assert!((rate - 10.0).abs() < 1e-10);
    }

    #[test]
    fn rate_detector_negative_ramp() {
        let mut r = RateDetector::new(1.0);
        r.update(5.0, 0.1);
        let rate = r.update(4.0, 0.1);
        assert!((rate - (-10.0)).abs() < 1e-10);
    }

    #[test]
    fn rate_detector_smoothing_reduces_spike() {
        let mut r_raw = RateDetector::new(1.0);
        let mut r_smooth = RateDetector::new(0.3);
        r_raw.update(0.0, 0.1);
        r_smooth.update(0.0, 0.1);
        let raw = r_raw.update(10.0, 0.1);
        let smooth = r_smooth.update(10.0, 0.1);
        // Both should detect the spike, but smooth version is attenuated
        assert!(smooth.abs() < raw.abs());
        assert!(smooth > 0.0);
    }

    #[test]
    fn rate_detector_reset_returns_zero() {
        let mut r = RateDetector::new(1.0);
        r.update(0.0, 0.1);
        r.update(10.0, 0.1);
        r.reset();
        assert_eq!(r.update(5.0, 0.1), 0.0);
    }

    // ── Property-based tests ────────────────────────────────────────────────

    proptest! {
        #[test]
        fn lpf_bounded_by_prev_and_input(
            prev in -1000.0_f64..1000.0_f64,
            input in -1000.0_f64..1000.0_f64,
            alpha in 0.01_f64..1.0_f64,
        ) {
            let mut f = LowPassFilter::new(alpha);
            f.update(prev);
            let out = f.update(input);
            let lo = prev.min(input);
            let hi = prev.max(input);
            prop_assert!(out >= lo - 1e-10 && out <= hi + 1e-10);
        }

        #[test]
        fn lpf_after_reset_next_equals_input(
            prev in -1000.0_f64..1000.0_f64,
            next in -1000.0_f64..1000.0_f64,
            alpha in 0.01_f64..1.0_f64,
        ) {
            let mut f = LowPassFilter::new(alpha);
            f.update(prev);
            f.reset();
            let out = f.update(next);
            prop_assert!((out - next).abs() < 1e-12);
        }

        /// CircularFilter output is always in [-π, π].
        #[test]
        fn circular_output_in_range(
            a1 in -PI..PI,
            a2 in -PI..PI,
            alpha in 0.01_f64..1.0_f64,
        ) {
            let mut f = CircularFilter::new(alpha);
            f.update(a1);
            let out = f.update(a2);
            prop_assert!((-PI - 1e-10..=PI + 1e-10).contains(&out),
                "CircularFilter output {out} not in [-π, π]");
        }
    }
}
