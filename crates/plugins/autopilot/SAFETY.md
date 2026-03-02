# Autopilot Safety Analysis

> **Scope**: `crates/plugins/autopilot` — software PID autopilot running as a
> Tier-1 plugin inside signalk-rs. Outputs `steering.rudderAngle` to the SK
> store; hardware actuation (if any) is performed by a separate driver that
> reads from the store.
>
> **Disclaimer**: This document is an informal hazard analysis for a
> development project. It does **not** claim compliance with IEC 61508,
> ISO 17894, or any other functional safety standard. Never use autopilot
> software as the sole safety system on a vessel. A human watch-keeper must
> always be present and ready to override.

---

## Guiding Principle

**The autopilot controls the rudder angle stored in the SK data model.**
It does not communicate directly with hardware. If the hardware driver or
actuator fails, the autopilot continues to write to the store but the rudder
does not respond — the boat will veer off course. This must be monitored by
the operator.

The corollary: the most important safety mechanism is a **physical manual
override** (tiller/wheel) that is always available and takes precedence over
any electronic command.

---

## Hazard Analysis (HAZOP-style)

| # | Hazard | Root Cause | Effect | Mitigation | Test Coverage |
|---|--------|-----------|--------|------------|---------------|
| H1 | Unintended steering | Engagement with no target set | No rudder command (target=None guard) | Control loop skips if target is None | `provider::tests::*` |
| H2 | Wrong direction at ±180° boundary | Angle wrap arithmetic | Steering the long way around (358° instead of 2°) | `normalize_angle()` applied at every arithmetic step | `pd::tests::normalize_*`, `simulation::compass_wraps_*` |
| H3 | Heading sensor loss | GPS off, compass unplugged | Autopilot continues with stale data | `sensor_timed_out()` watchdog: disengages + Alarm after 10s | `state::sensor_timed_out()`, control loop timeout |
| H4 | Noisy sensor / wind glitch at ±π | Cheap compass, wind crossing dead upwind | Motor hunts; wind filter glitches through 0° | Dead zone (±1°); D-term; **CircularFilter** (sin/cos separate) for wind | `simulation::compass_stable_no_hunting`, `filter::tests::circular_stays_near_pi_*` |
| H5 | Overshoot causes dangerous gybe | High P-gain with insufficient D | Boat turns through dead downwind, uncontrolled gybe | D-term damps convergence; yaw-rate D-term preferred | `simulation::compass_no_significant_overshoot` |
| H6 | Runaway rudder | Gains too high, integral windup | Rapid oscillation, structural stress | Rudder clamped ±max_rudder_rad (30°); rate limiting (0.09 rad/s) | `pd::tests::clamps_to_max_rudder`, `proptest::pid_output_always_clamped` |
| H7 | NaN in arithmetic | Invalid sensor value (Inf, NaN) | Rudder NaN → hardware garbage | `is_finite()` guard in `PidController::compute()` → return 0.0 | `pd::tests::nan_error_returns_zero_rudder` |
| H8 | Tack in wrong mode | User calls tack in compass mode | 404 error, no maneuver | `tack()`/`gybe()` check `mode == Wind`, return `not_found` | `provider::tests::tack_requires_wind_mode` |
| H9 | Route mode diverges | Large initial XTE, no convergence | Boat circles | Cascaded LOS guidance: outer loop (atan-bounded), inner heading PID; experimental (feature-gated) | `route::tests::*`, `route::tests::heading_correction_bounded_by_atan` |
| H10 | Plugin crash corrupts state | Panic in async task | Control loop dies, rudder holds last angle | Tokio task isolation; PID reset on timeout/disengage | Plugin lifecycle tests |
| H11 | Integral windup | I-term accumulates during prolonged saturation | Delayed recovery when error reverses | Anti-windup: integral frozen when output saturated; integral_limit cap; dead-zone decay (×0.95/tick); reset on mode change/disengage | `pd::tests::anti_windup_*`, `pd::tests::integral_decays_in_dead_zone` |
| H12 | Heel feedforward wrong sign | User configures positive heel_gain | Rudder amplifies weather helm instead of countering it | Default −0.5 (correct for most yachts); documentation warns about sign | Config documentation |
| H13 | Recovery mode overshoot | Boosted gains (×2) during recovery | Boat overshoots target heading after large deviation | Timeout (15 ticks max); early exit when error < 30% threshold; I-term disabled during recovery | `simulation::recovery_no_excessive_overshoot`, `pd::tests::recovery_deactivates_on_timeout` |
| H14 | Gust response wrong sign | Positive gust_gain with typical rig | Rudder amplifies gust-induced heel/turn | Default −0.02 (correct for most yachts); threshold prevents response to normal variation | `simulation::gust_feedforward_reduces_peak_deviation` |
| H15 | Rudder not responding | Stuck actuator, broken hydraulic, driver crash | Autopilot commands rudder but boat doesn't turn | `RudderFeedbackMonitor`: alarm after 30 consecutive mismatch ticks (3s); source-filtered subscription (NMEA sensor vs autopilot output); dormant without sensor | `pd::tests::feedback_*`, `simulation::rudder_feedback_*` |
| H16 | Heading sensor glitch | EMI spike, compass failure, connector fault | Sudden 180° heading jump → D-term spike → full rudder wrong direction | `HeadingPlausibility`: rejects delta > `max_heading_rate` × dt; 1 glitch → discard + use prev heading; 3 consecutive → SensorFailure → disengage + Alarm + rudder=0 | `pd::tests::plausibility_*` (7 unit), `simulation::heading_glitch_*` (2 integration) |
| H17 | Stale sensor D-term amplification | Low sensor update rate (1 Hz compass on 10 Hz loop) | Inaccurate rate-of-change → D-term oscillation or spike | `sensor_quality()`: D-gain × 2^(−age/half_life); `validate_yaw_rate()`: reject >0.8 rad/s → finite-diff fallback | `pd::tests::quality_*` (6 unit), `pd::tests::yaw_rate_*` (6 unit), `simulation::stale_sensor_*`, `simulation::yaw_rate_glitch_*` |

---

## Known Limitations

### Route mode convergence (H9)
Route mode uses cascaded LOS guidance (outer XTE→heading, inner heading PID).
The `atan(XTE/lookahead)` outer loop naturally saturates at ±π/2, preventing
runaway corrections. Unit tests verify directionality and bounding. A full
2D trajectory simulation (vessel moving along a track with lateral offset)
remains pending.

### Recovery mode overshoot (H13)
Recovery mode boosts P and D gains by 2× and disables the I-term for at most
15 ticks (1.5s at 10 Hz). The boosted D-term limits overshoot. Simulation
tests verify overshoot stays below 15°. If gains are too aggressive for a
specific vessel, reduce `recovery_gain_factor` or set `recovery_threshold_rad`
to 0 to disable.

### Gust response sign convention (H14)
The `gust_gain` sign must be negative for typical sailing yachts (bear away
from gust to spill wind). A positive value would steer into the gust. The
`gust_threshold_mps_per_sec` (default 3.0 m/s²) prevents response to normal
wind variation.

### Rudder feedback sensor dependency (H15)
The `RudderFeedbackMonitor` requires a hardware rudder sensor sending NMEA RSA
(0183) or PGN 127245 (2000) data to `steering.rudderAngle`. Without this sensor,
`actual_rudder_rad` stays `None` and the monitor is dormant — no detection of
stuck rudder is possible. The autopilot does not auto-disengage on feedback
failure (it emits an alarm); the operator must act. Source filtering ensures the
autopilot's own output to `steering.rudderAngle` is not mistaken for sensor feedback.

### Heel compensation sign convention (H12)
The `heel_gain` sign must be negative to counter weather helm on a standard
sailing yacht. A positive value would amplify the effect. There is no
automatic sign detection — the user must verify correct operation after
engaging autopilot with heel compensation.

---

## Failure Mode Response Matrix

| Failure | Response | How triggered |
|---------|----------|--------------|
| Sensor timeout | `enabled = false`, PID reset, emit Alarm, emit `rudderAngle = 0.0` | `sensor_timed_out()` in control loop |
| Unknown mode from API | `PluginError::not_found` returned, no state change | `AutopilotMode::from_str()` parse error |
| No target set when engaged | Control loop tick skips (no rudder command) | `target_rad.is_none()` check |
| No sensor value when enabled | Control loop tick skips | `primary_sensor.is_none()` check |
| Mode change | PID integral reset, wind filter reset, recovery reset | `prev_mode != mode` detection |
| Large sudden deviation | Recovery mode: boost P,D ×2, disable I, max 15 ticks | `RecoveryState::update()` in control loop |
| Wind gust detected | Preemptive rudder via `gust_gain × d(AWS)/dt` | `RateDetector` + threshold in control loop |
| NaN sensor value | Zero rudder output for that tick | `is_finite()` guard in PidController |
| Rudder feedback mismatch | Alarm notification (`steering.autopilot.rudderFeedbackFailure`), status update; auto-clears when rudder responds | `RudderFeedbackMonitor` in control loop; requires NMEA rudder sensor |
| Hardware driver stops | Rudder feedback alarm (if sensor available); sensor timeout fires if heading also stops | `RudderFeedbackMonitor` (H15), Watchdog (H3) |
| Single heading glitch | Discard spike, use previous heading, emit warning | `HeadingPlausibility` in control loop (H16) |
| Consecutive heading failure | `enabled = false`, PID reset, all monitors reset, emit Alarm (`steering.autopilot.headingSensorFailure`), `rudderAngle = 0.0` | `HeadingPlausibility` consecutive glitch count ≥ `heading_glitch_max_count` (H16) |
| Stale heading data | D-gain attenuated by `sensor_quality()`; P and I remain full strength | `dterm_quality_half_life_secs` > 0 in control loop (H17) |
| Implausible yaw rate | Value discarded → `None` → finite-difference D-term fallback (no disengage) | `validate_yaw_rate()` in control loop (H17) |

---

## Test Coverage Targets

Every hazard in the HAZOP table should have at least one automated test.

| Hazard | Status | Test(s) |
|--------|--------|---------|
| H1: No target guard | ✅ | `provider::tests::engage_sets_enabled` |
| H2: Angle wrap | ✅ | `pd::tests::normalize_*`, `simulation::compass_wraps_*` |
| H3: Sensor timeout | ⚠️ | `state::sensor_timed_out()` tests; control loop integration missing |
| H4: Hunting + wind filter | ✅ | `simulation::compass_stable_no_hunting`, `filter::tests::circular_stays_near_pi_*` |
| H5: Overshoot | ✅ | `simulation::compass_no_significant_overshoot` |
| H6: Runaway rudder | ✅ | `pd::tests::clamps_to_max_rudder`, `proptest::pid_output_always_clamped` |
| H7: NaN propagation | ✅ | `pd::tests::nan_error_returns_zero_rudder` |
| H8: Tack mode check | ✅ | `provider::tests::tack_requires_wind_mode` |
| H9: Route convergence | ⚠️ | `route::tests::*` (unit); 2D trajectory sim pending |
| H10: Plugin crash | ⚠️ | Covered by Tokio task isolation; no explicit test |
| H11: Integral windup | ✅ | `pd::tests::anti_windup_*`, `pd::tests::integral_decays_in_dead_zone`, `pd::tests::integral_clamped_to_limit` |
| H12: Heel feedforward | ⚠️ | Config validation; no automated sign-check test |
| H13: Recovery overshoot | ✅ | `simulation::recovery_no_excessive_overshoot`, `pd::tests::recovery_*` |
| H14: Gust response sign | ✅ | `simulation::gust_feedforward_reduces_peak_deviation` |
| H15: Rudder not responding | ✅ | `pd::tests::feedback_*` (8 unit), `simulation::rudder_feedback_*` (4 integration) |
| H16: Heading sensor glitch | ✅ | `pd::tests::plausibility_*` (7 unit), `simulation::heading_glitch_*` (2 integration) |
| H17: Stale sensor D-term | ✅ | `pd::tests::quality_*` (6 unit), `pd::tests::yaw_rate_*` (6 unit), `simulation::stale_sensor_*`, `simulation::yaw_rate_glitch_*` (2 proptest) |

Legend: ✅ covered · ⚠️ partial / gap

---

## Operational Safety Rules

1. **Always have a human watch-keeper** who can override by wheel/tiller.
2. **Test in calm conditions** before relying on the autopilot in challenging situations.
3. **Verify rudder response** after each autopilot engagement.
4. **Set appropriate sensor timeouts** for your hardware (default: 10 s).
5. **Do not use route mode** (experimental) on a production vessel without extensive testing.
6. **Monitor the `steering.autopilot.dataTimeout` notification** — it indicates disengagement.
7. **Verify heel compensation sign** — heel_gain should be negative for standard yachts.
8. **Verify gust response sign** — gust_gain should be negative for standard yachts.
9. **Test recovery mode behaviour** — if overshoot is excessive, reduce `recovery_gain_factor` or disable (`recovery_threshold_rad = 0`).
10. **Install a rudder feedback sensor** (NMEA RSA or PGN 127245) for hardware monitoring — without it, the autopilot cannot detect a stuck rudder.
11. **Monitor `steering.autopilot.rudderFeedbackFailure`** — it indicates the rudder is not responding to commands.
12. **Monitor `steering.autopilot.headingSensorFailure`** — it indicates the heading sensor is producing implausible data and the autopilot has disengaged.
13. **Ensure heading sensor update rate matches control rate** — a 1 Hz compass on a 10 Hz loop causes D-term attenuation via `dterm_quality_half_life_secs`. Upgrade sensor or reduce `control_rate_hz`.

---

## Future Safety Improvements

- [x] NaN/Inf guard in PID controller (H7)
- [x] Circular (sin/cos) low-pass filter for wind angle (H4)
- [x] Anti-windup with integral limit (H11)
- [x] Rudder rate limiting (H6 enhancement)
- [x] Recovery mode: boost gains for large deviations, timeout + early exit (H13)
- [x] Gust response: wind speed feedforward for proactive correction (H14)
- [x] Cascaded route controller: outer XTE→heading, inner heading PID (H9 improvement)
- [x] TWA wind mode: `environment.wind.angleTrue` (experimental)
- [x] Rudder feedback monitoring: compare commanded vs. actual from NMEA sensor (H15)
- [x] Heading plausibility check: reject deltas > max_turn_rate (detect sensor glitches) (H16)
- [x] Sensor quality metric: weight D-term by sensor update frequency (H17)
- [ ] 2D route trajectory simulation (H9 gap)
- [ ] IEC 61508 SIL assessment (formal, if commercially deployed)
