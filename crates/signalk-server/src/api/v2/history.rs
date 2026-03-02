//! SignalK v2 History API handlers.
//!
//! - `GET /signalk/v2/api/history/values`
//! - `GET /signalk/v2/api/history/contexts`
//! - `GET /signalk/v2/api/history/paths`

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::ServerState;
use crate::history::query::{
    AggregateMethod, ContextsRequest, PathSpec, PathsRequest, ValuesRequest,
};

/// Query parameters for `/signalk/v2/api/history/values`.
#[derive(Debug, Deserialize)]
pub struct ValuesParams {
    /// Comma-separated paths, optionally with `:method` suffix.
    /// e.g. `navigation.speedOverGround:average,navigation.courseOverGroundTrue`
    #[serde(default)]
    pub paths: Option<String>,
    /// Context (default: `vessels.self`).
    #[serde(default)]
    pub context: Option<String>,
    /// Desired time resolution: "1s", "1m", "1h", "1d", or milliseconds.
    #[serde(default)]
    pub resolution: Option<String>,
    /// Start of time range (ISO 8601).
    #[serde(default)]
    pub from: Option<String>,
    /// End of time range (ISO 8601).
    #[serde(default)]
    pub to: Option<String>,
    /// Duration (ISO 8601 or milliseconds).
    #[serde(default)]
    pub duration: Option<String>,
}

/// Query parameters for `/signalk/v2/api/history/contexts`.
#[derive(Debug, Deserialize)]
pub struct ContextsParams {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub duration: Option<String>,
}

/// Query parameters for `/signalk/v2/api/history/paths`.
#[derive(Debug, Deserialize)]
pub struct PathsParams {
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub duration: Option<String>,
}

/// Parse a comma-separated paths string into `PathSpec` entries.
///
/// Each path can have an optional `:method` suffix:
/// `navigation.speedOverGround:average` or just `navigation.speedOverGround`.
fn parse_path_specs(paths_str: &str) -> Vec<PathSpec> {
    paths_str
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|entry| {
            let entry = entry.trim();
            if let Some((path, method_str)) = entry.rsplit_once(':') {
                PathSpec {
                    path: path.to_string(),
                    method: AggregateMethod::parse_method(method_str),
                }
            } else {
                PathSpec {
                    path: entry.to_string(),
                    method: AggregateMethod::default(),
                }
            }
        })
        .collect()
}

/// `GET /signalk/v2/api/history/values`
pub async fn get_values(
    State(state): State<Arc<ServerState>>,
    Query(params): Query<ValuesParams>,
) -> Response {
    let provider = state.history_manager.provider().await;

    let paths_str = match params.paths {
        Some(p) if !p.is_empty() => p,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"message": "Missing required parameter: paths"})),
            )
                .into_response();
        }
    };

    let path_specs = parse_path_specs(&paths_str);
    if path_specs.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": "No valid paths specified"})),
        )
            .into_response();
    }

    let req = ValuesRequest {
        context: params.context.unwrap_or_else(|| "vessels.self".to_string()),
        path_specs,
        from: params.from,
        to: params.to,
        duration: params.duration,
        resolution: params.resolution,
    };

    // Run the (synchronous) query on a blocking thread
    let result = tokio::task::spawn_blocking(move || provider.get_values(&req))
        .await
        .map_err(|e| format!("join: {e}"));

    match result {
        Ok(Ok(resp)) => Json(resp).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": e})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": e})),
        )
            .into_response(),
    }
}

/// `GET /signalk/v2/api/history/contexts`
pub async fn get_contexts(
    State(state): State<Arc<ServerState>>,
    Query(params): Query<ContextsParams>,
) -> Response {
    let provider = state.history_manager.provider().await;

    let req = ContextsRequest {
        from: params.from,
        to: params.to,
        duration: params.duration,
    };

    let result = tokio::task::spawn_blocking(move || provider.get_contexts(&req))
        .await
        .map_err(|e| format!("join: {e}"));

    match result {
        Ok(Ok(contexts)) => Json(contexts).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": e})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": e})),
        )
            .into_response(),
    }
}

/// `GET /signalk/v2/api/history/paths`
pub async fn get_paths(
    State(state): State<Arc<ServerState>>,
    Query(params): Query<PathsParams>,
) -> Response {
    let provider = state.history_manager.provider().await;

    let req = PathsRequest {
        context: params.context.unwrap_or_else(|| "vessels.self".to_string()),
        from: params.from,
        to: params.to,
        duration: params.duration,
    };

    let result = tokio::task::spawn_blocking(move || provider.get_paths(&req))
        .await
        .map_err(|e| format!("join: {e}"));

    match result {
        Ok(Ok(paths)) => Json(paths).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"message": e})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": e})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_paths_with_methods() {
        let specs =
            parse_path_specs("navigation.speedOverGround:average,navigation.courseOverGroundTrue");
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].path, "navigation.speedOverGround");
        assert_eq!(specs[0].method, AggregateMethod::Average);
        assert_eq!(specs[1].path, "navigation.courseOverGroundTrue");
        assert_eq!(specs[1].method, AggregateMethod::Average); // default
    }

    #[test]
    fn parse_paths_various_methods() {
        let specs = parse_path_specs("a:min, b:max, c:count");
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].method, AggregateMethod::Min);
        assert_eq!(specs[1].method, AggregateMethod::Max);
        assert_eq!(specs[2].method, AggregateMethod::Count);
    }

    #[test]
    fn parse_empty_paths() {
        let specs = parse_path_specs("");
        assert!(specs.is_empty());
    }
}
