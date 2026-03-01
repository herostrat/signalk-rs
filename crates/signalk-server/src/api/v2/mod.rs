/// SignalK v2 REST API handlers.
///
/// Routes:
/// - GET /signalk/v2/features                           → feature discovery
/// - GET/POST /signalk/v2/api/resources/{type}          → resource listing/creation
/// - GET/PUT/DELETE /signalk/v2/api/resources/{type}/{id} → resource CRUD
/// - GET/DELETE /signalk/v2/api/vessels/self/navigation/course → course state
/// - GET/POST/DELETE .../course/_config[/apiOnly]       → course configuration
/// - PUT .../course/destination                         → set destination
/// - PUT .../course/activeRoute                         → follow route
/// - POST /signalk/v2/api/notifications/{id}/silence    → silence alarm
/// - POST /signalk/v2/api/notifications/{id}/acknowledge → acknowledge alarm
pub mod course;
pub mod features;
pub mod notifications;
pub mod resources;
