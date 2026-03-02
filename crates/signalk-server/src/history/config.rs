//! History subsystem configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the history subsystem.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(default)]
pub struct HistoryConfig {
    /// Whether history recording is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Sampling interval in seconds. Deltas are batched and written at this rate.
    #[serde(default = "default_sampling_interval")]
    pub sampling_interval_secs: f64,

    /// Glob patterns for paths to include (default: all).
    #[serde(default = "default_include")]
    pub include: Vec<String>,

    /// Glob patterns for paths to exclude (default: none).
    #[serde(default)]
    pub exclude: Vec<String>,

    /// Retention for raw data in days (default: 3).
    #[serde(default = "default_retention_raw")]
    pub retention_raw_days: u32,

    /// Retention for daily aggregates in days (default: 90).
    #[serde(default = "default_retention_daily")]
    pub retention_daily_days: u32,

    /// How often to run aggregation/pruning in seconds (default: 300 = 5 min).
    #[serde(default = "default_aggregation_interval")]
    pub aggregation_interval_secs: u64,

    /// Maximum database size in MB (default: 500). When exceeded, oldest raw
    /// data is pruned aggressively until size is under limit.
    #[serde(default = "default_max_db_size_mb")]
    pub max_db_size_mb: u64,

    /// Run VACUUM after pruning to reclaim disk space (default: true).
    #[serde(default = "default_vacuum_after_prune")]
    pub vacuum_after_prune: bool,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            enabled: default_enabled(),
            sampling_interval_secs: default_sampling_interval(),
            include: default_include(),
            exclude: Vec::new(),
            retention_raw_days: default_retention_raw(),
            retention_daily_days: default_retention_daily(),
            aggregation_interval_secs: default_aggregation_interval(),
            max_db_size_mb: default_max_db_size_mb(),
            vacuum_after_prune: default_vacuum_after_prune(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_sampling_interval() -> f64 {
    1.0
}

fn default_include() -> Vec<String> {
    vec!["**".to_string()]
}

fn default_retention_raw() -> u32 {
    3
}

fn default_retention_daily() -> u32 {
    90
}

fn default_aggregation_interval() -> u64 {
    300
}

fn default_max_db_size_mb() -> u64 {
    500
}

fn default_vacuum_after_prune() -> bool {
    true
}

/// Simple glob matching for SignalK paths.
///
/// Supports `*` (single segment) and `**` (all remaining segments).
pub fn path_matches_glob(path: &str, pattern: &str) -> bool {
    if pattern == "**" {
        return true;
    }

    let path_parts: Vec<&str> = path.split('.').collect();
    let pattern_parts: Vec<&str> = pattern.split('.').collect();

    match_parts(&path_parts, &pattern_parts)
}

fn match_parts(path: &[&str], pattern: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }
    if pattern[0] == "**" {
        // ** matches zero or more segments
        if pattern.len() == 1 {
            return true;
        }
        for i in 0..=path.len() {
            if match_parts(&path[i..], &pattern[1..]) {
                return true;
            }
        }
        return false;
    }
    if path.is_empty() {
        return false;
    }
    if pattern[0] == "*" || pattern[0] == path[0] {
        return match_parts(&path[1..], &pattern[1..]);
    }
    false
}

/// Check if a path should be recorded based on include/exclude patterns.
pub fn should_record(path: &str, include: &[String], exclude: &[String]) -> bool {
    // Check exclude first — any match means skip
    for ex in exclude {
        if path_matches_glob(path, ex) {
            return false;
        }
    }
    // Check include — at least one must match
    for inc in include {
        if path_matches_glob(path, inc) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HistoryConfig::default();
        assert!(cfg.enabled);
        assert!((cfg.sampling_interval_secs - 1.0).abs() < 1e-10);
        assert_eq!(cfg.include, vec!["**"]);
        assert!(cfg.exclude.is_empty());
        assert_eq!(cfg.retention_raw_days, 3);
        assert_eq!(cfg.retention_daily_days, 90);
        assert_eq!(cfg.aggregation_interval_secs, 300);
        assert_eq!(cfg.max_db_size_mb, 500);
        assert!(cfg.vacuum_after_prune);
    }

    #[test]
    fn glob_star_star_matches_everything() {
        assert!(path_matches_glob("navigation.speedOverGround", "**"));
        assert!(path_matches_glob("a.b.c.d", "**"));
        assert!(path_matches_glob("x", "**"));
    }

    #[test]
    fn glob_exact_match() {
        assert!(path_matches_glob(
            "navigation.speedOverGround",
            "navigation.speedOverGround"
        ));
        assert!(!path_matches_glob(
            "navigation.speedOverGround",
            "navigation.courseOverGround"
        ));
    }

    #[test]
    fn glob_single_star() {
        assert!(path_matches_glob(
            "navigation.speedOverGround",
            "navigation.*"
        ));
        assert!(!path_matches_glob(
            "navigation.position.latitude",
            "navigation.*"
        ));
    }

    #[test]
    fn glob_prefix_star_star() {
        assert!(path_matches_glob(
            "propulsion.main.revolutions",
            "propulsion.**"
        ));
        assert!(path_matches_glob(
            "propulsion.port.temperature",
            "propulsion.**"
        ));
        assert!(!path_matches_glob("navigation.speed", "propulsion.**"));
    }

    #[test]
    fn glob_mixed() {
        assert!(path_matches_glob(
            "environment.wind.speedApparent",
            "environment.wind.*"
        ));
        assert!(!path_matches_glob(
            "environment.depth.belowTransducer",
            "environment.wind.*"
        ));
    }

    #[test]
    fn should_record_default_includes_all() {
        let include = vec!["**".to_string()];
        let exclude: Vec<String> = vec![];
        assert!(should_record(
            "navigation.speedOverGround",
            &include,
            &exclude
        ));
    }

    #[test]
    fn should_record_exclude_takes_precedence() {
        let include = vec!["**".to_string()];
        let exclude = vec!["propulsion.**".to_string()];
        assert!(should_record(
            "navigation.speedOverGround",
            &include,
            &exclude
        ));
        assert!(!should_record(
            "propulsion.main.revolutions",
            &include,
            &exclude
        ));
    }

    #[test]
    fn should_record_specific_include() {
        let include = vec!["navigation.*".to_string(), "environment.wind.*".to_string()];
        let exclude: Vec<String> = vec![];
        assert!(should_record(
            "navigation.speedOverGround",
            &include,
            &exclude
        ));
        assert!(should_record(
            "environment.wind.speedApparent",
            &include,
            &exclude
        ));
        assert!(!should_record(
            "propulsion.main.revolutions",
            &include,
            &exclude
        ));
    }
}
