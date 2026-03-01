# Autopilot Safety Analysis

> **Scope**: `crates/plugins/autopilot` — software PD autopilot running as a
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
| H1 | Unintended steering | Engagement with no target set | No rudder command (target=None guard), boat drifts | Control loop skips if target is None | `provider::tests::*` |
| H2 | Wrong direction at ±180° boundary | Angle wrap arithmetic | Steering the long way around (358° instead of 2°) | `normalize_angle()` applied at every arithmetic step | `pd::tests::normalize_*`, `simulation::compass_wraps_correctly_at_180_boundary` |
| H3 | Heading sensor loss | GPS off, compass unplugged, serial disconnect | Autopilot continues commanding rudder with stale data | `sensor_timed_out()` watchdog: disengages and emits Alarm notification after N seconds (default 10) | `state::tests::*`, timeout logic in control loop |
| H4 | Noisy sensor driving hunting | Cheap compass, electrical interference | Motor hunts back and forth, battery drain, mechanical wear | Dead zone (±1° default); D-term dampens; wind angle low-pass filtered | `simulation::compass_stable_no_hunting_after_convergence` |
| H5 | Overshoot causes dangerous gybe | High P-gain with no D-term | Boat turns through dead downwind, uncontrolled gybe | D-term damps convergence; yaw-rate feedback preferred over finite diff | `simulation::compass_no_significant_overshoot` |
| H6 | Runaway rudder | gain_p or gain_d set too high | Rapid oscillation, structural stress on rudder | Rudder clamped to ±max_rudder_rad (default 30°) by `compute_rudder()` | `pd::tests::clamps_to_max_rudder`, `proptest::rudder_always_clamped` |
| H7 | Integer overflow / NaN in arithmetic | Invalid sensor value (Inf, NaN) | Rudder NaN → SK store stores NaN, hardware sees garbage | f64 NaN/Inf checks: `pv.value.as_f64()` returns None for non-finite JSON numbers; dead zone check `error_rad.abs() < dead_zone` would be false for NaN → P+D formula runs, but `clamp(-max, +max)` with NaN returns NaN | **GAP: add `is_finite()` guard in compute_rudder** |
| H8 | Tack in wrong mode | User calls tack in compass mode | 404 error returned, no maneuver | `tack()` / `gybe()` check `mode == Wind`, return `not_found` otherwise | `provider::tests::tack_requires_wind_mode` |
| H9 | Route mode diverges | Large initial XTE, no convergence | Boat circles; never reaches waypoint | XTE correction clamped to ±max_rudder_rad; route mode is experimental (feature-gated) | `simulation::*` (route-specific tests needed in Phase C+) |
| H10 | Plugin crash corrupts state | Panic in async task | Control loop dies, rudder holds last commanded angle | Tokio task isolation: plugin crash doesn't crash the server; `stop()` aborts the task | Plugin lifecycle tests |

---

## Known Limitations

### ~~NaN / non-finite sensor values (H7)~~ — **Fixed**

`compute_rudder()` now guards against NaN/Inf inputs at the top of the function.
If either `error_rad` or `d_error_rad` is non-finite, zero is returned immediately
— no correction applied, no NaN propagated to the SK store or hardware driver.
Covered by `pd::tests::nan_error_returns_zero_rudder`.

### Route mode convergence (H9)
Route mode (experimental) lacks dedicated simulation tests for XTE
convergence. The LOS guidance formula is correct in theory but has not been
tested with a realistic trajectory simulation.

### Circular wind angle filtering (H4)
The wind angle low-pass filter operates on the raw radian value. If the wind
veers rapidly from +π to -π (dead-upwind), the filter interpolates through 0°
(wrong direction). This is a known limitation of linear filtering on angular
quantities. A future fix should filter sin/cos components separately.

---

## Failure Mode Response Matrix

| Failure | Response | How triggered |
|---------|----------|--------------|
| Sensor timeout | `enabled = false`, emit Alarm notification, emit `steering.rudderAngle = 0.0` | `sensor_timed_out()` in control loop |
| Unknown mode from API | `PluginError::not_found` returned, no state change | `AutopilotMode::from_str()` parse error |
| No target set when engaged | Control loop tick skips (no rudder command) | `target_rad.is_none()` check |
| No sensor value when enabled | Control loop tick skips | `primary_sensor.is_none()` check |
| Hardware driver stops reading | No detection at autopilot level; sensor timeout will fire if heading also stops | Watchdog (H3) |

---

## Test Coverage Targets

Every hazard in the HAZOP table should have at least one automated test.

| Hazard | Status | Test(s) |
|--------|--------|---------|
| H1: No target guard | ✅ | `provider::tests::engage_sets_enabled` (target=None → no rudder) |
| H2: Angle wrap | ✅ | `pd::tests::normalize_*`, `simulation::compass_wraps_correctly_at_180_boundary` |
| H3: Sensor timeout | ⚠️ | `state.rs::sensor_timed_out()` tests; control loop integration missing |
| H4: Hunting | ✅ | `simulation::compass_stable_no_hunting_after_convergence` |
| H5: Overshoot | ✅ | `simulation::compass_no_significant_overshoot` |
| H6: Runaway rudder | ✅ | `pd::tests::clamps_to_max_rudder`, `proptest::rudder_always_clamped` |
| H7: NaN propagation | ✅ | `pd::tests::nan_error_returns_zero_rudder` |
| H8: Tack mode check | ✅ | `provider::tests::tack_requires_wind_mode` |
| H9: Route convergence | ⚠️ | Experimental feature; simulation tests for route pending |
| H10: Plugin crash | ⚠️ | Covered by Tokio task isolation; no explicit test |

Legend: ✅ covered · ⚠️ partial / gap · ❌ missing

---

## Operational Safety Rules

1. **Always have a human watch-keeper** who can override by wheel/tiller.
2. **Test in calm conditions** before relying on the autopilot in challenging situations.
3. **Verify rudder response** after each autopilot engagement (check the actual rudder moves with commanded angle).
4. **Set appropriate sensor timeouts** for your hardware (default: 10 s; reduce if sensors are reliable, increase for slow GPS fix).
5. **Do not use route mode** (experimental) on a production vessel without extensive testing.
6. **Monitor the `steering.autopilot.dataTimeout` notification** — it indicates the autopilot has disengaged due to sensor loss.

---

## Future Safety Improvements

- [x] NaN/Inf guard in `compute_rudder()` (H7) — done
- [ ] Circular (sin/cos) low-pass filter for wind angle (H4)
- [ ] Rudder feedback monitoring: compare commanded vs. actual `steering.rudderAngle.actual`
- [ ] Heading plausibility check: reject deltas > max_turn_rate (detect sensor glitches)
- [ ] Sensor quality metric: weight D-term by sensor update frequency
- [ ] IEC 61508 SIL assessment (formal, if commercially deployed)
