/// Resource REST API handlers.
///
/// | Method | Route | Handler |
/// |--------|-------|---------|
/// | GET    | `/signalk/v2/api/resources/{type}` | `list_resources` |
/// | POST   | `/signalk/v2/api/resources/{type}` | `create_resource` |
/// | GET    | `/signalk/v2/api/resources/{type}/{id}` | `get_resource` |
/// | PUT    | `/signalk/v2/api/resources/{type}/{id}` | `update_resource` |
/// | DELETE | `/signalk/v2/api/resources/{type}/{id}` | `delete_resource` |
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use signalk_types::resources::ResourceType;
use signalk_types::v2::{ResourceQueryParams, ResourceResponse};
use std::sync::Arc;

use crate::ServerState;

/// Validate that the resource type is one of the 5 standard types.
fn validate_resource_type(type_name: &str) -> Result<(), (StatusCode, String)> {
    match ResourceType::parse(type_name) {
        Some(_) => Ok(()),
        None => Err((
            StatusCode::NOT_FOUND,
            format!("Unknown resource type: {type_name}"),
        )),
    }
}

/// `GET /signalk/v2/api/resources/{type}`
pub async fn list_resources(
    State(state): State<Arc<ServerState>>,
    Path(resource_type): Path<String>,
    Query(query): Query<ResourceQueryParams>,
) -> impl IntoResponse {
    if let Err(e) = validate_resource_type(&resource_type) {
        return e.into_response();
    }

    match state.resource_providers.list(&resource_type, &query).await {
        Ok(resources) => Json(resources).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /signalk/v2/api/resources/{type}`
pub async fn create_resource(
    State(state): State<Arc<ServerState>>,
    Path(resource_type): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = validate_resource_type(&resource_type) {
        return e.into_response();
    }

    match state.resource_providers.create(&resource_type, body).await {
        Ok(id) => Json(ResourceResponse {
            state: "COMPLETED".into(),
            status_code: 200,
            id,
        })
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /signalk/v2/api/resources/{type}/{id}`
pub async fn get_resource(
    State(state): State<Arc<ServerState>>,
    Path((resource_type, id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = validate_resource_type(&resource_type) {
        return e.into_response();
    }

    match state.resource_providers.get(&resource_type, &id).await {
        Ok(Some(resource)) => Json(resource).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /signalk/v2/api/resources/{type}/{id}`
pub async fn update_resource(
    State(state): State<Arc<ServerState>>,
    Path((resource_type, id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = validate_resource_type(&resource_type) {
        return e.into_response();
    }

    match state
        .resource_providers
        .update(&resource_type, &id, body)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) if e.to_string().contains("not found") => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `DELETE /signalk/v2/api/resources/{type}/{id}`
pub async fn delete_resource(
    State(state): State<Arc<ServerState>>,
    Path((resource_type, id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = validate_resource_type(&resource_type) {
        return e.into_response();
    }

    match state.resource_providers.delete(&resource_type, &id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) if e.to_string().contains("not found") => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
