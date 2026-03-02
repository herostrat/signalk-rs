//! History API request/response types.

use serde::{Deserialize, Serialize};

/// Request for `/signalk/v2/api/history/values`.
#[derive(Debug, Clone)]
pub struct ValuesRequest {
    /// Context to query (default: `vessels.self`).
    pub context: String,
    /// Paths to query, each with an optional aggregation method.
    pub path_specs: Vec<PathSpec>,
    /// Start of time range.
    pub from: Option<String>,
    /// End of time range (default: now).
    pub to: Option<String>,
    /// ISO 8601 duration or milliseconds.
    pub duration: Option<String>,
    /// Desired time resolution (e.g. "1s", "1m", "1h", "1d" or milliseconds).
    pub resolution: Option<String>,
}

/// A single path + aggregation method.
#[derive(Debug, Clone)]
pub struct PathSpec {
    pub path: String,
    pub method: AggregateMethod,
}

/// Supported aggregation methods.
///
/// See SignalK History API spec:
/// `average | min | max | first | last | mid | middle_index`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AggregateMethod {
    #[default]
    Average,
    Min,
    Max,
    First,
    Last,
    Count,
    /// Median value within the time bucket.
    Mid,
    /// Value at the middle timestamp of the time bucket.
    MiddleIndex,
}

impl AggregateMethod {
    pub fn parse_method(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "min" => Self::Min,
            "max" => Self::Max,
            "first" => Self::First,
            "last" => Self::Last,
            "count" => Self::Count,
            "mid" => Self::Mid,
            "middle_index" | "middleindex" => Self::MiddleIndex,
            _ => Self::Average,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Average => "average",
            Self::Min => "min",
            Self::Max => "max",
            Self::First => "first",
            Self::Last => "last",
            Self::Count => "count",
            Self::Mid => "mid",
            Self::MiddleIndex => "middle_index",
        }
    }
}

/// Auto-determine a reasonable resolution in seconds based on the time range.
///
/// Aims for roughly 500–1000 data points. The spec says: "If resolution is
/// not specified the server should provide data in a reasonable time
/// resolution, depending on the time range in the request."
pub fn auto_resolution_secs(range_secs: f64) -> Option<f64> {
    if range_secs <= 0.0 {
        return None;
    }
    // Target ~500 data points
    let bucket = range_secs / 500.0;
    if bucket < 1.0 {
        // Less than 500 seconds total — return full resolution
        return None;
    }
    // Snap to nice boundaries
    if bucket < 5.0 {
        Some(1.0)
    } else if bucket < 30.0 {
        Some(10.0)
    } else if bucket < 120.0 {
        Some(60.0) // 1m
    } else if bucket < 600.0 {
        Some(300.0) // 5m
    } else if bucket < 3600.0 {
        Some(900.0) // 15m
    } else if bucket < 14400.0 {
        Some(3600.0) // 1h
    } else {
        Some(86400.0) // 1d
    }
}

/// Response for `/signalk/v2/api/history/values`.
#[derive(Debug, Clone, Serialize)]
pub struct ValuesResponse {
    pub context: String,
    pub range: TimeRange,
    pub values: Vec<ValueMeta>,
    /// Each row: `[timestamp, value1, value2, ...]`.
    pub data: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimeRange {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValueMeta {
    pub path: String,
    pub method: String,
}

/// Request for `/signalk/v2/api/history/contexts`.
#[derive(Debug, Clone)]
pub struct ContextsRequest {
    pub from: Option<String>,
    pub to: Option<String>,
    pub duration: Option<String>,
}

/// Request for `/signalk/v2/api/history/paths`.
#[derive(Debug, Clone)]
pub struct PathsRequest {
    pub context: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub duration: Option<String>,
}

/// Parse a duration string to seconds.
///
/// Accepts: ISO 8601 durations (PT1H, P1D, PT30M) or milliseconds as integer.
pub fn parse_duration_secs(s: &str) -> Option<f64> {
    // Try as milliseconds first
    if let Ok(ms) = s.parse::<f64>() {
        return Some(ms / 1000.0);
    }

    // Simple ISO 8601 duration parser
    let s = s.trim();
    if !s.starts_with('P') {
        return None;
    }
    let s = &s[1..]; // strip P

    let mut total_secs = 0.0;
    let mut in_time = false;
    let mut num_start = 0;

    for (i, c) in s.char_indices() {
        match c {
            'T' => {
                in_time = true;
                num_start = i + 1;
            }
            'D' if !in_time => {
                let n: f64 = s[num_start..i].parse().ok()?;
                total_secs += n * 86400.0;
                num_start = i + 1;
            }
            'H' if in_time => {
                let n: f64 = s[num_start..i].parse().ok()?;
                total_secs += n * 3600.0;
                num_start = i + 1;
            }
            'M' if in_time => {
                let n: f64 = s[num_start..i].parse().ok()?;
                total_secs += n * 60.0;
                num_start = i + 1;
            }
            'S' if in_time => {
                let n: f64 = s[num_start..i].parse().ok()?;
                total_secs += n;
                num_start = i + 1;
            }
            _ => {}
        }
    }

    if total_secs > 0.0 {
        Some(total_secs)
    } else {
        None
    }
}

/// Parse a resolution string to seconds.
///
/// Accepts: "1s", "1m", "1h", "1d", or milliseconds.
pub fn parse_resolution_secs(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('s') {
        return n.parse::<f64>().ok();
    }
    if let Some(n) = s.strip_suffix('m') {
        return n.parse::<f64>().ok().map(|v| v * 60.0);
    }
    if let Some(n) = s.strip_suffix('h') {
        return n.parse::<f64>().ok().map(|v| v * 3600.0);
    }
    if let Some(n) = s.strip_suffix('d') {
        return n.parse::<f64>().ok().map(|v| v * 86400.0);
    }
    // Milliseconds
    s.parse::<f64>().ok().map(|ms| ms / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_iso_duration() {
        assert!((parse_duration_secs("PT1H").unwrap() - 3600.0).abs() < 0.1);
        assert!((parse_duration_secs("PT30M").unwrap() - 1800.0).abs() < 0.1);
        assert!((parse_duration_secs("P1D").unwrap() - 86400.0).abs() < 0.1);
        assert!((parse_duration_secs("P1DT6H").unwrap() - 108000.0).abs() < 0.1);
        assert!((parse_duration_secs("PT1H30M").unwrap() - 5400.0).abs() < 0.1);
    }

    #[test]
    fn parse_millisecond_duration() {
        assert!((parse_duration_secs("60000").unwrap() - 60.0).abs() < 0.1);
        assert!((parse_duration_secs("1000").unwrap() - 1.0).abs() < 0.1);
    }

    #[test]
    fn parse_invalid_duration() {
        assert!(parse_duration_secs("foo").is_none());
        assert!(parse_duration_secs("").is_none());
    }

    #[test]
    fn parse_resolution_units() {
        assert!((parse_resolution_secs("1s").unwrap() - 1.0).abs() < 0.1);
        assert!((parse_resolution_secs("5m").unwrap() - 300.0).abs() < 0.1);
        assert!((parse_resolution_secs("1h").unwrap() - 3600.0).abs() < 0.1);
        assert!((parse_resolution_secs("1d").unwrap() - 86400.0).abs() < 0.1);
    }

    #[test]
    fn parse_resolution_ms() {
        assert!((parse_resolution_secs("100").unwrap() - 0.1).abs() < 0.01);
        assert!((parse_resolution_secs("1000").unwrap() - 1.0).abs() < 0.01);
    }

    #[test]
    fn aggregate_method_roundtrip() {
        assert_eq!(AggregateMethod::parse_method("average").as_str(), "average");
        assert_eq!(AggregateMethod::parse_method("min").as_str(), "min");
        assert_eq!(AggregateMethod::parse_method("MAX").as_str(), "max");
        assert_eq!(AggregateMethod::parse_method("unknown").as_str(), "average");
        assert_eq!(AggregateMethod::parse_method("mid").as_str(), "mid");
        assert_eq!(
            AggregateMethod::parse_method("middle_index").as_str(),
            "middle_index"
        );
        assert_eq!(
            AggregateMethod::parse_method("middleindex").as_str(),
            "middle_index"
        );
    }

    #[test]
    fn auto_resolution_short_range() {
        // 60 seconds — should be full resolution (None)
        assert!(auto_resolution_secs(60.0).is_none());
        // 300 seconds — still < 500 points at 1/s
        assert!(auto_resolution_secs(300.0).is_none());
    }

    #[test]
    fn auto_resolution_medium_range() {
        // 1 hour = 3600s → ~500 points at ~7s → snaps to 10s
        let r = auto_resolution_secs(3600.0).unwrap();
        assert!(
            (1.0..=60.0).contains(&r),
            "1h range: expected 1-60s, got {r}"
        );
    }

    #[test]
    fn auto_resolution_long_range() {
        // 24 hours = 86400s → ~500 points at ~172s → snaps to 300s (5m)
        let r = auto_resolution_secs(86400.0).unwrap();
        assert!(
            (60.0..=900.0).contains(&r),
            "24h range: expected 60-900s, got {r}"
        );

        // 30 days → snaps to 1h or 1d
        let r = auto_resolution_secs(30.0 * 86400.0).unwrap();
        assert!(r >= 3600.0, "30d range: expected >=3600s, got {r}");
    }
}
