/// Autopilot control mode implementations.
///
/// Each mode encapsulates the control law for a specific steering strategy.
/// The control loop in `lib.rs` reads sensor values from the SignalK store,
/// dispatches to the active mode, and emits the resulting rudder command.
///
/// # Mode stability classification
///
/// | Module    | Mode string | Status   | Notes                          |
/// |-----------|-------------|----------|--------------------------------|
/// | `heading` | "compass"   | ✅ Stable | Heading hold, PD controller    |
/// | `wind`    | "wind"      | 🧪 Experimental | AWA hold, Low-pass filtered |
/// | `route`   | "route"     | 🧪 Experimental | LOS guidance + XTE correction |
///
/// Experimental modes are gated behind the `autopilot-experimental` feature
/// (see `signalk-server/Cargo.toml`).
pub mod heading;
