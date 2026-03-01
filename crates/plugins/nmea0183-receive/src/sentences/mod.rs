//! NMEA 0183 sentence → SignalK Delta conversion.
//!
//! Each sub-module handles a thematic group of sentences and exports
//! `pub fn from_xxx(data: &XxxData) -> Vec<PathValue>` converters.
//!
//! Unit conversions follow the SignalK specification:
//!   - Speed  : knots     → m/s   (× 0.514 444)
//!   - Angles : degrees   → radians
//!   - Depth  : meters    (already SI)
//!   - Lat/Lon: decimal degrees (no radian conversion — SK spec uses degrees for position)
use serde_json::Value;

/// One SignalK path-value pair extracted from a sentence.
pub struct PathValue {
    pub path: String,
    pub value: Value,
}

impl PathValue {
    pub(crate) fn new(path: impl Into<String>, value: Value) -> Self {
        PathValue {
            path: path.into(),
            value,
        }
    }
}

mod course;
mod depth;
mod environment;
mod gnss;
mod heading;
mod position;
mod propulsion;
mod wind;

pub use course::*;
pub use depth::*;
pub use environment::*;
pub use gnss::*;
pub use heading::*;
pub use position::*;
pub use propulsion::*;
pub use wind::*;
