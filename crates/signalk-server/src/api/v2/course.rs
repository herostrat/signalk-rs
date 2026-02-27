/// Course REST API handlers.
///
/// All under `/signalk/v2/api/vessels/self/navigation/course`.
///
/// | Method | Route | Handler |
/// |--------|-------|---------|
/// | GET    | `.../course` | `get_course` |
/// | DELETE | `.../course` | `clear_course` |
/// | PUT    | `.../course/destination` | `set_destination` |
/// | PUT    | `.../course/activeRoute` | `set_active_route` |
/// | PUT    | `.../course/activeRoute/nextPoint` | `advance_next_point` |
/// | PUT    | `.../course/activeRoute/pointIndex` | `set_point_index` |
/// | PUT    | `.../course/activeRoute/reverse` | `reverse_route` |
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
