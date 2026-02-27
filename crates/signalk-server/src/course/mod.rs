/// Course/navigation management.
///
/// The `CourseManager` stores active navigation state (destination, active route,
/// arrival circle) and emits raw course deltas into the SignalK store.
/// Derived values (bearing, distance, XTE) are computed by separate calculators
/// in the derived-data plugin.
pub mod manager;

pub use manager::CourseManager;
