/// Autopilot control mode implementations.
///
/// Each mode encapsulates the control law for a specific steering strategy.
/// The control loop in `lib.rs` reads sensor values from the SignalK store,
/// dispatches to the active mode, and emits the resulting rudder command.
///
/// # Mode stability classification
///
/// | Module     | Mode string  | Status          | Notes                              |
/// |------------|--------------|-----------------|-------------------------------------|
/// | `heading`  | "compass"    | Stable          | Heading hold, PID controller        |
/// | `wind`     | "wind"       | Stable          | AWA hold, CircularFilter + PID      |
/// | `wind`     | "wind_true"  | Experimental    | TWA hold, CircularFilter + PID      |
/// | `route`    | "route"      | Experimental    | Cascaded LOS guidance + heading PID |
///
/// Experimental modes are gated behind the `autopilot-experimental` feature
/// (see `signalk-server/Cargo.toml`).
pub mod heading;
#[cfg(feature = "experimental")]
pub mod route;
