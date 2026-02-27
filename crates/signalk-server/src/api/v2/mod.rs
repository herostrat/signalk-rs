/// SignalK v2 REST API handlers.
///
/// Routes:
/// - GET /signalk/v2/features                           → feature discovery
/// - GET/POST /signalk/v2/api/resources/{type}          → resource listing/creation
/// - GET/PUT/DELETE /signalk/v2/api/resources/{type}/{id} → resource CRUD
/// - GET/DELETE /signalk/v2/api/vessels/self/navigation/course → course state
/// - PUT .../course/destination                         → set destination
/// - PUT .../course/activeRoute                         → follow route
pub mod course;
pub mod features;
pub mod resources;
