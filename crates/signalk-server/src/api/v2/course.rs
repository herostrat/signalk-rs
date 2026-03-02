/// Course REST API handlers.
///
/// All under `/signalk/v2/api/vessels/self/navigation/course`.
///
/// | Method | Route | Handler |
/// |--------|-------|---------|
/// | GET    | `.../course` | `get_course` |
/// | GET    | `.../course/_config` | `get_config` |
/// | GET    | `.../course/calcValues` | `get_calc_values` |
/// | DELETE | `.../course` | `clear_course` |
/// | POST   | `.../course/_config/apiOnly` | `enable_api_only` |
/// | DELETE | `.../course/_config/apiOnly` | `disable_api_only` |
/// | PUT    | `.../course/destination` | `set_destination` |
/// | PUT    | `.../course/activeRoute` | `set_active_route` |
/// | PUT    | `.../course/activeRoute/nextPoint` | `advance_next_point` |
/// | PUT    | `.../course/activeRoute/pointIndex` | `set_point_index` |
/// | PUT    | `.../course/activeRoute/reverse` | `reverse_route` |
/// | PUT    | `.../course/targetArrivalTime` | `set_target_arrival_time` |
/// | PUT    | `.../course/arrivalCircle` | `set_arrival_circle` |
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use signalk_types::v2::{
    ActiveRouteRequest, DestinationRequest, PointAdvanceRequest, PointIndexRequest,
};
use std::sync::Arc;

use crate::ServerState;

/// `GET /signalk/v2/api/vessels/self/navigation/course`
pub async fn get_course(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    match state.course_manager.get_state().await {
        Some(course_state) => Json(course_state).into_response(),
        None => Json(serde_json::json!({})).into_response(),
    }
}

/// `DELETE /signalk/v2/api/vessels/self/navigation/course`
pub async fn clear_course(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    match state.course_manager.clear().await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /signalk/v2/api/vessels/self/navigation/course/destination`
pub async fn set_destination(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<DestinationRequest>,
) -> impl IntoResponse {
    match state.course_manager.set_destination(req).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) if e.to_string().contains("not found") => {
            (StatusCode::NOT_FOUND, e.to_string()).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// `PUT /signalk/v2/api/vessels/self/navigation/course/activeRoute`
pub async fn set_active_route(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<ActiveRouteRequest>,
) -> impl IntoResponse {
    match state.course_manager.set_active_route(req).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) if e.to_string().contains("not found") => {
            (StatusCode::NOT_FOUND, e.to_string()).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// `PUT /signalk/v2/api/vessels/self/navigation/course/activeRoute/nextPoint`
pub async fn advance_next_point(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<PointAdvanceRequest>,
) -> impl IntoResponse {
    match state.course_manager.advance_next_point(req.value).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// `PUT /signalk/v2/api/vessels/self/navigation/course/activeRoute/pointIndex`
pub async fn set_point_index(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<PointIndexRequest>,
) -> impl IntoResponse {
    match state.course_manager.set_point_index(req.value).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// `PUT /signalk/v2/api/vessels/self/navigation/course/activeRoute/reverse`
pub async fn reverse_route(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    match state.course_manager.reverse_route().await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// `PUT /signalk/v2/api/vessels/self/navigation/course/targetArrivalTime`
pub async fn set_target_arrival_time(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let time = req
        .get("value")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    match state.course_manager.set_target_arrival_time(time).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// `PUT /signalk/v2/api/vessels/self/navigation/course/arrivalCircle`
pub async fn set_arrival_circle(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let radius = match req.get("value").and_then(|v| v.as_f64()) {
        Some(r) if r >= 0.0 => r,
        _ => return (StatusCode::BAD_REQUEST, "Invalid arrival circle radius").into_response(),
    };
    match state.course_manager.set_arrival_circle(radius).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /signalk/v2/api/vessels/self/navigation/course/calcValues`
pub async fn get_calc_values(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    Json(state.course_manager.get_calc_values().await)
}

/// `GET /signalk/v2/api/vessels/self/navigation/course/_config`
pub async fn get_config(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    Json(state.course_manager.get_config().await)
}

/// `POST /signalk/v2/api/vessels/self/navigation/course/_config/apiOnly`
pub async fn enable_api_only(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    state.course_manager.enable_api_only().await;
    StatusCode::OK
}

/// `DELETE /signalk/v2/api/vessels/self/navigation/course/_config/apiOnly`
pub async fn disable_api_only(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    state.course_manager.disable_api_only().await;
    StatusCode::OK
}
