//! Utilities for working with SignalK dot-separated paths.
//!
//! Paths always refer to leaf nodes: "navigation.speedOverGround".
//! Context is separate from paths in deltas.

/// Split a dot-path into its components.
pub fn split(path: &str) -> Vec<&str> {
    path.split('.').collect()
}

/// Join path components with dots.
pub fn join(parts: &[&str]) -> String {
    parts.join(".")
}

/// Normalize a context: "vessels.self" stays as-is,
/// bare "self" becomes "vessels.self".
pub fn normalize_context(context: &str) -> &str {
    if context == "self" {
        "vessels.self"
    } else {
        context
    }
}

/// Check if a path pattern (with wildcards) matches a concrete path.
///
/// Supports:
/// - `*` matches exactly one segment
/// - `**` or trailing `.*` matches any number of remaining segments
///
/// Examples:
/// - `"navigation.*"` matches `"navigation.speedOverGround"`
/// - `"propulsion.*.oilTemperature"` matches `"propulsion.main.oilTemperature"`
/// - `"*"` matches `"navigation.speedOverGround"` (any single segment) — NO, only same depth
pub fn matches_pattern(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('.').collect();
    let path_parts: Vec<&str> = path.split('.').collect();
    matches_parts(&pattern_parts, &path_parts)
}

fn matches_parts(pattern: &[&str], path: &[&str]) -> bool {
    match (pattern.first(), path.first()) {
        (None, None) => true,
        (Some(&"**"), _) => true, // ** matches any remaining
        (None, _) | (_, None) => false,
        (Some(&"*"), _) => matches_parts(&pattern[1..], &path[1..]),
        (Some(p), Some(q)) if p == q => matches_parts(&pattern[1..], &path[1..]),
        _ => false,
    }
}

/// Resolve "vessels.self" to the actual vessel UUID in a full context path.
pub fn resolve_self(context: &str, self_uri: &str) -> String {
    if context == "vessels.self" {
        format!("vessels.{}", self_uri)
    } else {
        context.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_join_roundtrip() {
        let path = "navigation.speedOverGround";
        let parts = split(path);
        assert_eq!(parts, vec!["navigation", "speedOverGround"]);
        assert_eq!(join(&parts), path);
    }

    #[test]
    fn pattern_exact() {
        assert!(matches_pattern(
            "navigation.speedOverGround",
            "navigation.speedOverGround"
        ));
        assert!(!matches_pattern(
            "navigation.speedOverGround",
            "navigation.courseOverGroundTrue"
        ));
    }

    #[test]
    fn pattern_single_wildcard() {
        assert!(matches_pattern(
            "navigation.*",
            "navigation.speedOverGround"
        ));
        assert!(matches_pattern(
            "navigation.*",
            "navigation.courseOverGroundTrue"
        ));
        assert!(!matches_pattern(
            "navigation.*",
            "propulsion.speedOverGround"
        ));
        // Wildcard does NOT match across segments
        assert!(!matches_pattern(
            "navigation.*",
            "navigation.position.latitude"
        ));
    }

    #[test]
    fn pattern_mid_wildcard() {
        assert!(matches_pattern(
            "propulsion.*.oilTemperature",
            "propulsion.main.oilTemperature"
        ));
        assert!(matches_pattern(
            "propulsion.*.oilTemperature",
            "propulsion.aux.oilTemperature"
        ));
        assert!(!matches_pattern(
            "propulsion.*.oilTemperature",
            "propulsion.main.oilPressure"
        ));
    }

    #[test]
    fn pattern_double_wildcard() {
        assert!(matches_pattern(
            "navigation.**",
            "navigation.speedOverGround"
        ));
        assert!(matches_pattern(
            "navigation.**",
            "navigation.position.latitude"
        ));
        assert!(matches_pattern("**", "anything.goes.here"));
    }

    #[test]
    fn normalize_context_self() {
        assert_eq!(normalize_context("self"), "vessels.self");
        assert_eq!(normalize_context("vessels.self"), "vessels.self");
        assert_eq!(
            normalize_context("vessels.urn:mrn:signalk:uuid:abc"),
            "vessels.urn:mrn:signalk:uuid:abc"
        );
    }
}
