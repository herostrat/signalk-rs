/// Value generators for the simulator plugin.
///
/// All generators are deterministic (based on elapsed time) — no external
/// `rand` crate needed. Pseudo-noise uses a simple hash-based approach.
use std::f64::consts::PI;
use std::time::Instant;

// ─── Pseudo-random noise (no external rand dependency) ──────────────────────

/// Simple hash-based pseudo-random number generator.
/// Returns a value in [0, 1) for a given seed.
fn hash_f64(seed: u64) -> f64 {
    // SplitMix64-style mixing
    let mut z = seed.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z = z ^ (z >> 31);
    (z as f64) / (u64::MAX as f64)
}

/// Approximate normal distribution using Central Limit Theorem.
/// Returns a value roughly in [-1, 1] (actually [-3σ, +3σ] of a narrow normal).
fn pseudo_normal(seed: u64) -> f64 {
    let sum: f64 = (0..6).map(|i| hash_f64(seed.wrapping_add(i * 7919))).sum();
    // 6 uniform [0,1) values: mean=3.0, stddev≈0.7071
    // Normalize to roughly [-1, 1]
    (sum - 3.0) / 1.5
}

// ─── SineGenerator ──────────────────────────────────────────────────────────

/// Smooth oscillation between `min` and `max` with a given period.
///
/// ```text
/// t = (elapsed % period) / period     // 0..1
/// value = min + 0.5 * (1 - cos(t * 2π)) * (max - min)
/// ```
pub struct SineGenerator {
    min: f64,
    max: f64,
    period_secs: f64,
    /// Phase offset in seconds — shifts the start of the wave so that
    /// multiple generators with the same period don't move in lock-step.
    phase_offset_secs: f64,
}

impl SineGenerator {
    pub fn new(min: f64, max: f64, period_secs: f64) -> Self {
        SineGenerator {
            min,
            max,
            period_secs,
            phase_offset_secs: 0.0,
        }
    }

    pub fn with_phase(mut self, phase_secs: f64) -> Self {
        self.phase_offset_secs = phase_secs;
        self
    }

    pub fn value(&self, elapsed_secs: f64) -> f64 {
        let t = ((elapsed_secs + self.phase_offset_secs) % self.period_secs) / self.period_secs;
        self.min + 0.5 * (1.0 - (t * 2.0 * PI).cos()) * (self.max - self.min)
    }
}

// ─── NoiseGenerator ─────────────────────────────────────────────────────────

/// Adds pseudo-random noise to a base value.
///
/// Uses time-quantized steps so the noise changes smoothly-ish
/// (new random value every `step_secs` seconds, linearly interpolated).
pub struct NoiseGenerator {
    amplitude: f64,
    step_secs: f64,
}

impl NoiseGenerator {
    pub fn new(amplitude: f64, step_secs: f64) -> Self {
        NoiseGenerator {
            amplitude,
            step_secs,
        }
    }

    /// Returns a noise offset in [-amplitude, +amplitude] for the given time.
    pub fn offset(&self, elapsed_secs: f64) -> f64 {
        let step = (elapsed_secs / self.step_secs).floor() as u64;
        let frac = (elapsed_secs / self.step_secs).fract();

        // Interpolate between two consecutive noise samples
        let a = pseudo_normal(step.wrapping_mul(6_364_136_223_846_793_005));
        let b = pseudo_normal(step.wrapping_add(1).wrapping_mul(6_364_136_223_846_793_005));

        (a + (b - a) * frac) * self.amplitude
    }
}

// ─── PositionGenerator (circular orbit) ─────────────────────────────────────

/// Simulates a vessel moving in a circle around a center point.
///
/// Produces position (lat/lon), course over ground, and speed over ground.
pub struct PositionGenerator {
    center_lat: f64,
    center_lon: f64,
    radius_deg: f64,
    orbit_period_secs: f64,
    /// Speed in m/s (circumference / period)
    speed_mps: f64,
}

/// Output of one position generator tick.
pub struct PositionOutput {
    pub latitude: f64,
    pub longitude: f64,
    /// Course over ground in radians [0, 2π)
    pub cog_rad: f64,
    /// Speed over ground in m/s
    pub sog_mps: f64,
}

impl PositionGenerator {
    /// Create a new position generator.
    ///
    /// - `center_lat`, `center_lon`: center of the circular orbit (degrees)
    /// - `radius_m`: orbit radius in meters
    /// - `orbit_period_secs`: time for one full circle
    pub fn new(center_lat: f64, center_lon: f64, radius_m: f64, orbit_period_secs: f64) -> Self {
        // Convert radius from meters to degrees (approximate)
        let radius_deg = radius_m / 111_320.0;
        // Circumference = 2π * radius_m, speed = circumference / period
        let speed_mps = 2.0 * PI * radius_m / orbit_period_secs;

        PositionGenerator {
            center_lat,
            center_lon,
            radius_deg,
            orbit_period_secs,
            speed_mps,
        }
    }

    pub fn generate(&self, elapsed_secs: f64) -> PositionOutput {
        let angle = (elapsed_secs / self.orbit_period_secs) * 2.0 * PI;

        let lat = self.center_lat + self.radius_deg * angle.cos();
        // Correct longitude for latitude (degrees get "wider" at equator)
        let lon =
            self.center_lon + self.radius_deg * angle.sin() / self.center_lat.to_radians().cos();

        // COG is tangent to the circle (perpendicular to radius vector)
        // At angle θ, tangent direction = θ + π/2
        let cog = (angle + PI / 2.0).rem_euclid(2.0 * PI);

        PositionOutput {
            latitude: lat,
            longitude: lon,
            cog_rad: cog,
            sog_mps: self.speed_mps,
        }
    }
}

// ─── CorrelatedGenerator ────────────────────────────────────────────────────

/// Generates values that correlate with another input (e.g. RPM ~ SOG).
///
/// ```text
/// output = base + (input / max_input) * range + noise
/// ```
pub struct CorrelatedGenerator {
    base: f64,
    range: f64,
    max_input: f64,
    noise: NoiseGenerator,
}

impl CorrelatedGenerator {
    pub fn new(base: f64, range: f64, max_input: f64, noise_amplitude: f64) -> Self {
        CorrelatedGenerator {
            base,
            range,
            max_input,
            noise: NoiseGenerator::new(noise_amplitude, 2.0),
        }
    }

    pub fn value(&self, input: f64, elapsed_secs: f64) -> f64 {
        let ratio = (input / self.max_input).clamp(0.0, 1.0);
        self.base + ratio * self.range + self.noise.offset(elapsed_secs)
    }
}

// ─── SimulatorState ─────────────────────────────────────────────────────────

/// Holds all generators and produces a complete set of simulated values.
pub struct SimulatorState {
    start: Instant,
    pub position: PositionGenerator,

    // Navigation
    heading_noise: NoiseGenerator,
    magnetic_variation_rad: f64,

    // Environment - wind
    pub wind_angle: SineGenerator,
    pub wind_speed: SineGenerator,
    wind_speed_noise: NoiseGenerator,

    // Environment - depth & water
    pub depth: SineGenerator,
    pub water_temp: SineGenerator,

    // Environment - atmosphere
    pub air_temp: SineGenerator,
    pub pressure: SineGenerator,
    pub humidity: SineGenerator,

    // Propulsion
    pub enable_propulsion: bool,
    pub rpm: CorrelatedGenerator,
    pub oil_temp: SineGenerator,
    oil_temp_noise: NoiseGenerator,
    pub coolant_temp: SineGenerator,
}

/// All simulated values for one tick.
pub struct SimulatedValues {
    // Navigation
    pub latitude: f64,
    pub longitude: f64,
    pub sog_mps: f64,
    pub cog_rad: f64,
    pub heading_magnetic_rad: f64,

    // Environment - wind
    pub wind_angle_apparent_rad: f64,
    pub wind_speed_apparent_mps: f64,

    // Environment - depth & water
    pub depth_below_transducer_m: f64,
    pub water_temperature_k: f64,

    // Environment - atmosphere
    pub air_temperature_k: f64,
    pub pressure_pa: f64,
    /// Relative humidity as ratio (0.0–1.0)
    pub humidity_ratio: f64,

    // Navigation - magnetic variation (emitted as own path for derived-data)
    pub magnetic_variation_rad: f64,

    // Propulsion (None if disabled)
    pub propulsion: Option<PropulsionValues>,
}

pub struct PropulsionValues {
    /// Engine revolutions in Hz (1/s)
    pub revolutions_hz: f64,
    /// Oil temperature in Kelvin
    pub oil_temperature_k: f64,
    /// Coolant temperature in Kelvin
    pub coolant_temperature_k: f64,
}

impl SimulatorState {
    pub fn new(
        center_lat: f64,
        center_lon: f64,
        orbit_radius_m: f64,
        orbit_period_secs: f64,
        magnetic_variation_deg: f64,
        enable_propulsion: bool,
    ) -> Self {
        SimulatorState {
            start: Instant::now(),
            position: PositionGenerator::new(
                center_lat,
                center_lon,
                orbit_radius_m,
                orbit_period_secs,
            ),

            // Heading: COG + compass jitter (±3°)
            heading_noise: NoiseGenerator::new(3.0_f64.to_radians(), 1.5),
            magnetic_variation_rad: magnetic_variation_deg.to_radians(),

            // Apparent wind angle: oscillates -π to π, period 30s
            wind_angle: SineGenerator::new(-PI, PI, 30.0),
            // Apparent wind speed: 2–15 m/s, period 25s
            wind_speed: SineGenerator::new(2.0, 15.0, 25.0).with_phase(5.0),
            wind_speed_noise: NoiseGenerator::new(1.0, 3.0),

            // Depth: 3–25m, slow oscillation (120s)
            depth: SineGenerator::new(3.0, 25.0, 120.0).with_phase(10.0),

            // Water temp: 283–293 K (10–20°C), very slow (600s)
            water_temp: SineGenerator::new(283.0, 293.0, 600.0).with_phase(30.0),

            // Air temp: 288–298 K (15–25°C), very slow (900s)
            air_temp: SineGenerator::new(288.0, 298.0, 900.0).with_phase(50.0),

            // Barometric pressure: 100800–102000 Pa, very slow (1800s)
            pressure: SineGenerator::new(100_800.0, 102_000.0, 1800.0).with_phase(100.0),

            // Relative humidity: 0.40–0.90 (40%–90%), very slow (1200s)
            humidity: SineGenerator::new(0.40, 0.90, 1200.0).with_phase(70.0),

            // Propulsion
            enable_propulsion,
            // RPM: base 5 Hz, up to +45 Hz correlated with SOG (max ~4 m/s)
            rpm: CorrelatedGenerator::new(5.0, 45.0, 4.0, 0.5),
            // Oil temp: 340–370 K, period 300s (warm-up cycle)
            oil_temp: SineGenerator::new(340.0, 370.0, 300.0).with_phase(20.0),
            oil_temp_noise: NoiseGenerator::new(2.0, 5.0),
            // Coolant temp: 340–365 K, period 360s
            coolant_temp: SineGenerator::new(340.0, 365.0, 360.0).with_phase(40.0),
        }
    }

    /// Generate all values for a given elapsed time (seconds since start).
    ///
    /// This is the core generation method — deterministic for the same `elapsed_secs`.
    /// Use this in tests for reproducible values.
    pub fn tick_at(&self, elapsed_secs: f64) -> SimulatedValues {
        // Position & navigation
        let pos = self.position.generate(elapsed_secs);

        // Heading magnetic = COG + variation + compass noise
        let heading_mag =
            (pos.cog_rad + self.magnetic_variation_rad + self.heading_noise.offset(elapsed_secs))
                .rem_euclid(2.0 * PI);

        // Environment
        let wind_speed_base = self.wind_speed.value(elapsed_secs);
        let wind_speed = (wind_speed_base + self.wind_speed_noise.offset(elapsed_secs)).max(0.0);

        // Propulsion
        let propulsion = if self.enable_propulsion {
            let revs = self.rpm.value(pos.sog_mps, elapsed_secs).max(0.0);
            let oil = self.oil_temp.value(elapsed_secs) + self.oil_temp_noise.offset(elapsed_secs);
            let coolant = self.coolant_temp.value(elapsed_secs);
            Some(PropulsionValues {
                revolutions_hz: revs,
                oil_temperature_k: oil,
                coolant_temperature_k: coolant,
            })
        } else {
            None
        };

        SimulatedValues {
            latitude: pos.latitude,
            longitude: pos.longitude,
            sog_mps: pos.sog_mps,
            cog_rad: pos.cog_rad,
            heading_magnetic_rad: heading_mag,
            wind_angle_apparent_rad: self.wind_angle.value(elapsed_secs),
            wind_speed_apparent_mps: wind_speed,
            depth_below_transducer_m: self.depth.value(elapsed_secs),
            water_temperature_k: self.water_temp.value(elapsed_secs),
            air_temperature_k: self.air_temp.value(elapsed_secs),
            pressure_pa: self.pressure.value(elapsed_secs),
            humidity_ratio: self.humidity.value(elapsed_secs),
            magnetic_variation_rad: self.magnetic_variation_rad,
            propulsion,
        }
    }

    /// Generate all values for the current instant.
    ///
    /// Convenience wrapper around `tick_at()` using wall-clock elapsed time.
    pub fn tick(&self) -> SimulatedValues {
        self.tick_at(self.start.elapsed().as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_generator_range() {
        let sg = SineGenerator::new(10.0, 20.0, 100.0);
        // Sample many points and verify they stay in [min, max]
        for i in 0..1000 {
            let t = i as f64 * 0.1;
            let v = sg.value(t);
            assert!(v >= 10.0 - f64::EPSILON, "value {v} below min at t={t}");
            assert!(v <= 20.0 + f64::EPSILON, "value {v} above max at t={t}");
        }
    }

    #[test]
    fn sine_generator_hits_extremes() {
        let sg = SineGenerator::new(0.0, 100.0, 100.0);
        // At t=0 (start of period), value should be at minimum
        let at_zero = sg.value(0.0);
        assert!((at_zero - 0.0).abs() < 0.01, "at t=0: {at_zero}");
        // At t=50 (half period), value should be at maximum
        let at_half = sg.value(50.0);
        assert!((at_half - 100.0).abs() < 0.01, "at t=50: {at_half}");
    }

    #[test]
    fn sine_generator_phase_offset() {
        let gen_a = SineGenerator::new(0.0, 100.0, 100.0);
        let gen_b = SineGenerator::new(0.0, 100.0, 100.0).with_phase(25.0);
        // gen_b at t=0 should equal gen_a at t=25
        let a_at_25 = gen_a.value(25.0);
        let b_at_0 = gen_b.value(0.0);
        assert!(
            (a_at_25 - b_at_0).abs() < 0.01,
            "a@25={a_at_25} vs b@0={b_at_0}"
        );
    }

    #[test]
    fn noise_generator_bounded() {
        let ng = NoiseGenerator::new(5.0, 1.0);
        for i in 0..1000 {
            let t = i as f64 * 0.05;
            let v = ng.offset(t);
            // pseudo_normal can exceed [-1,1] slightly, so allow 2x amplitude
            assert!(v.abs() <= 10.0, "noise {v} exceeds 2x amplitude at t={t}");
        }
    }

    #[test]
    fn position_generator_stays_near_center() {
        let pg = PositionGenerator::new(54.5, 10.0, 200.0, 300.0);
        for i in 0..300 {
            let pos = pg.generate(i as f64);
            let dlat = (pos.latitude - 54.5).abs();
            let dlon = (pos.longitude - 10.0).abs();
            assert!(dlat < 0.01, "lat drift too large: {dlat} at t={i}");
            assert!(dlon < 0.01, "lon drift too large: {dlon} at t={i}");
            assert!(pos.cog_rad >= 0.0 && pos.cog_rad < 2.0 * PI);
            assert!(pos.sog_mps > 0.0);
        }
    }

    #[test]
    fn position_generator_full_circle() {
        let pg = PositionGenerator::new(54.5, 10.0, 200.0, 300.0);
        let start = pg.generate(0.0);
        let end = pg.generate(300.0);
        // After one full period, should be back to start
        assert!(
            (start.latitude - end.latitude).abs() < 1e-6,
            "lat: {} vs {}",
            start.latitude,
            end.latitude
        );
        assert!(
            (start.longitude - end.longitude).abs() < 1e-6,
            "lon: {} vs {}",
            start.longitude,
            end.longitude
        );
    }

    #[test]
    fn correlated_generator_increases_with_input() {
        let cg = CorrelatedGenerator::new(5.0, 45.0, 4.0, 0.0);
        let low = cg.value(0.0, 0.0);
        let high = cg.value(4.0, 0.0);
        assert!(low < high, "low={low} should be < high={high}");
        assert!((low - 5.0).abs() < 0.01);
        assert!((high - 50.0).abs() < 0.01);
    }

    #[test]
    fn simulator_state_produces_valid_values() {
        let state = SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, true);
        let values = state.tick();

        assert!(values.latitude > 54.0 && values.latitude < 55.0);
        assert!(values.longitude > 9.5 && values.longitude < 10.5);
        assert!(values.sog_mps > 0.0);
        assert!(values.cog_rad >= 0.0 && values.cog_rad < 2.0 * PI);
        assert!(values.heading_magnetic_rad >= 0.0 && values.heading_magnetic_rad < 2.0 * PI);
        assert!(values.wind_speed_apparent_mps >= 0.0);
        assert!(values.depth_below_transducer_m >= 2.5);
        assert!(values.water_temperature_k > 280.0);
        assert!(values.air_temperature_k > 285.0);
        assert!(values.pressure_pa > 100_000.0);
        assert!(values.humidity_ratio >= 0.3 && values.humidity_ratio <= 1.0);
        assert!((values.magnetic_variation_rad - 2.5_f64.to_radians()).abs() < 0.01);
        assert!(values.propulsion.is_some());

        let prop = values.propulsion.unwrap();
        assert!(prop.revolutions_hz >= 0.0);
        assert!(prop.oil_temperature_k > 330.0);
        assert!(prop.coolant_temperature_k > 330.0);
    }

    #[test]
    fn simulator_state_no_propulsion() {
        let state = SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, false);
        let values = state.tick();
        assert!(values.propulsion.is_none());
    }

    #[test]
    fn tick_at_is_deterministic() {
        let state = SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, true);
        let a = state.tick_at(42.0);
        let b = state.tick_at(42.0);
        assert_eq!(a.latitude, b.latitude);
        assert_eq!(a.longitude, b.longitude);
        assert_eq!(a.sog_mps, b.sog_mps);
        assert_eq!(a.cog_rad, b.cog_rad);
        assert_eq!(a.heading_magnetic_rad, b.heading_magnetic_rad);
        assert_eq!(a.wind_angle_apparent_rad, b.wind_angle_apparent_rad);
        assert_eq!(a.pressure_pa, b.pressure_pa);
        assert_eq!(a.humidity_ratio, b.humidity_ratio);
        assert_eq!(a.magnetic_variation_rad, b.magnetic_variation_rad);
    }

    #[test]
    fn tick_at_different_times_differ() {
        let state = SimulatorState::new(54.5, 10.0, 200.0, 300.0, 2.5, true);
        let a = state.tick_at(0.0);
        let b = state.tick_at(10.0);
        // Position should differ after 10 seconds of orbiting
        assert_ne!(a.latitude, b.latitude);
        assert_ne!(a.cog_rad, b.cog_rad);
    }

    #[test]
    fn hash_f64_deterministic() {
        assert_eq!(hash_f64(42), hash_f64(42));
        assert_ne!(hash_f64(42), hash_f64(43));
    }

    #[test]
    fn pseudo_normal_distribution() {
        // Should mostly produce values in [-1, 1]
        let mut within_range = 0;
        for i in 0..1000 {
            let v = pseudo_normal(i);
            if v.abs() <= 1.0 {
                within_range += 1;
            }
        }
        // Most values (>80%) should be within [-1, 1]
        assert!(within_range > 800, "only {within_range}/1000 within [-1,1]");
    }
}
