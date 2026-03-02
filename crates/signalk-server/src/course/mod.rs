/// Course/navigation management.
///
/// The `CourseManager` stores active navigation state (destination, active route,
/// arrival circle) and emits raw course deltas into the SignalK store.
/// Derived values (bearing, distance, XTE) are computed by separate calculators
/// in the derived-data plugin.
///
/// # Data flow
///
/// ```text
///   Freeboard-SK / OpenCPN / Plotter App
///          |
///          v
///   ┌──────────────────┐     ┌─────────────────────┐
///   │ REST API (V2)    │     │ NMEA 0183/2000      │
///   │ PUT /course/...  │     │ RMB, BWC, XTE, APB  │
///   └────────┬─────────┘     └──────────┬──────────┘
///            │                          │
///            v                          v
///   ┌──────────────────┐     ┌─────────────────────┐
///   │  CourseManager   │◄────│  NMEA Listener      │
///   │  (state, persist)│     │  (V1 → V2 bridge)   │
///   └────────┬─────────┘     └─────────────────────┘
///            │
///            │ emit_deltas()
///            v
///   ┌──────────────────┐
///   │  SignalK Store   │  navigation.course.*
///   └────────┬─────────┘
///            │
///      ┌─────┴──────────┐
///      │                 │
///      v                 v
///   ┌──────────┐   ┌──────────────┐
///   │ Derived  │   │ Arrival      │
///   │ Data     │   │ Detection    │
///   │ Calcs    │   │ (→ advance)  │
///   └────┬─────┘   └──────────────┘
///        │
///        v
///   navigation.course.calcValues.*
///        │
///        v
///   ┌──────────────────┐
///   │ WS / REST / Apps │  (Monitoring + Autopilot)
///   └──────────────────┘
/// ```
///
/// # Autopilot integration
///
/// The autopilot plugin is a pure consumer of course data — it reads
/// `bearingTrackTrue` and `crossTrackError` from the store via subscriptions.
/// In route mode (experimental), a cascaded LOS controller converts XTE into
/// a heading correction, then feeds the standard heading PID as inner loop.
///
/// Waypoint advancement is handled by [`CourseManager::check_arrival()`], not
/// by the autopilot. When arrival is detected, the manager auto-advances to
/// the next waypoint and emits updated deltas — the autopilot follows
/// seamlessly without any mode switch.
pub mod manager;

pub use manager::CourseManager;
