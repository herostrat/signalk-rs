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
/// | `route`    | "route"      | Stable          | Cascaded LOS guidance + heading PID |
///
/// `wind_true` remains experimental (gated behind the `experimental` feature).
pub mod heading;
pub mod route;
