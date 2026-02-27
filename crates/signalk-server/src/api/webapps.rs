/// Webapp listing API handler.
use axum::{Json, extract::State, response::IntoResponse};
use std::sync::Arc;

use crate::ServerState;

/// GET /signalk/v1/webapps — list all discovered/registered webapps.
pub async fn list_webapps(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let registry = state.webapp_registry.read().await;
    Json(registry.all().to_vec())
}
