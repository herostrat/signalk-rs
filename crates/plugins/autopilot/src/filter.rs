/// First-order low-pass filter (exponential moving average).
///
/// Smooths a stream of noisy sensor values (e.g. compass heading, wind angle)
/// by applying an exponential moving average:
///
/// ```text
///   y[n] = y[n-1] + α * (x[n] - y[n-1])
/// ```
///
/// # Choosing alpha
///
/// Given a sampling interval `dt` (seconds) and desired time constant `τ` (seconds):
///
/// ```text
///   α = dt / (τ + dt)
/// ```
///
/// Examples (at 1 Hz):
/// - τ = 0.5 s → α ≈ 0.67  (light smoothing)
/// - τ = 2 s   → α = 0.33  (moderate smoothing, good for wind angle)
/// - τ = 10 s  → α ≈ 0.09  (heavy smoothing, good for very noisy signals)
///
/// Use `alpha = 1.0` for passthrough (no filtering).
pub struct LowPassFilter {
    value: Option<f64>,
    /// Smoothing factor: 0 < alpha ≤ 1 (smaller = more smoothing, slower response)
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
            None => input, // initialise to first sample
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn initializes_to_first_sample() {
        let mut f = LowPassFilter::new(0.5);
        assert_eq!(f.update(3.0), 3.0);
    }

    #[test]
    fn converges_toward_input() {
        let mut f = LowPassFilter::new(0.5);
        f.update(0.0); // initialise at 0
        let out = f.update(10.0);
        assert!(
            out > 0.0 && out < 10.0,
            "should be between 0 and 10, got {out}"
        );
    }

    #[test]
    fn passthrough_with_alpha_one() {
        let mut f = LowPassFilter::new(1.0);
        f.update(1.0);
        assert_eq!(f.update(42.0), 42.0);
    }

    #[test]
    fn output_stays_between_last_and_input() {
        let mut f = LowPassFilter::new(0.3);
        f.update(5.0); // initialise at 5
        let out = f.update(10.0);
        assert!((5.0..=10.0).contains(&out), "got {out}");
    }

    #[test]
    fn reset_reinitializes_to_next_sample() {
        let mut f = LowPassFilter::new(0.5);
        f.update(10.0);
        f.reset();
        assert_eq!(f.update(5.0), 5.0);
    }

    #[test]
    fn get_returns_none_before_first_update() {
        let f = LowPassFilter::new(0.5);
        assert!(f.get().is_none());
    }

    #[test]
    fn get_returns_value_after_update() {
        let mut f = LowPassFilter::new(0.5);
        f.update(7.0);
        assert_eq!(f.get(), Some(7.0));
    }

    #[test]
    fn heavy_smoothing_changes_slowly() {
        let mut f = LowPassFilter::new(0.05); // α = 0.05, τ ≈ 19 Δt
        f.update(0.0);
        let out = f.update(100.0);
        assert!(out < 10.0, "heavy filter should change slowly, got {out}");
    }

    // ── Property-based tests ───────────────────────────────────────────────────

    proptest! {
        /// After initialisation, the output is always between the previous output and the input.
        #[test]
        fn output_bounded_by_prev_and_input(
            prev in -1000.0_f64..1000.0_f64,
            input in -1000.0_f64..1000.0_f64,
            alpha in 0.01_f64..1.0_f64,
        ) {
            let mut f = LowPassFilter::new(alpha);
            f.update(prev); // initialise
            let out = f.update(input);
            let lo = prev.min(input);
            let hi = prev.max(input);
            prop_assert!(
                out >= lo - 1e-10 && out <= hi + 1e-10,
                "output {out} not in [{lo}, {hi}] (prev={prev}, input={input}, alpha={alpha})"
            );
        }

        /// Reset clears the state so the next update equals the input.
        #[test]
        fn after_reset_next_equals_input(
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
    }
}
