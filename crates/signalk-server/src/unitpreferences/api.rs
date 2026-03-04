use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::sync::Arc;

use super::UnitPreferencesManager;
use super::types::*;
use crate::ServerState;

fn unit_prefs(state: &ServerState) -> Result<&Arc<UnitPreferencesManager>, StatusCode> {
    state
        .unit_preferences
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

/// GET /signalk/v1/unitpreferences/config
pub async fn get_config(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    Ok::<_, StatusCode>(Json(mgr.config()))
}

/// PUT /signalk/v1/unitpreferences/config
pub async fn set_config(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<UnitPrefsConfig>,
) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    mgr.set_active_preset(&body.active_preset)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok::<_, StatusCode>(Json(mgr.config()))
}

/// GET /signalk/v1/unitpreferences/presets
pub async fn list_presets(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    Ok::<_, StatusCode>(Json(mgr.list_presets()))
}

/// GET /signalk/v1/unitpreferences/presets/:name
pub async fn get_preset(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    match mgr.get_preset(&name) {
        Ok(preset) => Ok(Json(preset)),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

/// PUT /signalk/v1/unitpreferences/presets/:name
pub async fn save_preset(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
    Json(preset): Json<Preset>,
) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    mgr.save_custom_preset(&name, &preset)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok::<_, StatusCode>(StatusCode::OK)
}

/// DELETE /signalk/v1/unitpreferences/presets/:name
pub async fn delete_preset(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    mgr.delete_custom_preset(&name)
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok::<_, StatusCode>(StatusCode::OK)
}

/// GET /signalk/v1/unitpreferences/active
pub async fn get_active(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    Ok::<_, StatusCode>(Json(mgr.active_preset()))
}

/// GET /signalk/v1/unitpreferences/definitions
pub async fn get_definitions(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    Ok::<_, StatusCode>(Json(mgr.definitions()))
}

/// GET /signalk/v1/unitpreferences/definitions/custom
pub async fn get_custom_definitions(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    Ok::<_, StatusCode>(Json(mgr.custom_definitions()))
}

/// PUT /signalk/v1/unitpreferences/definitions/custom
pub async fn set_custom_definitions(
    State(state): State<Arc<ServerState>>,
    Json(defs): Json<UnitDefinitions>,
) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    mgr.save_custom_definitions(&defs)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok::<_, StatusCode>(StatusCode::OK)
}

/// GET /signalk/v1/unitpreferences/categories
pub async fn get_categories(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    Ok::<_, StatusCode>(Json(mgr.categories()))
}

/// GET /signalk/v1/unitpreferences/categories/default
pub async fn get_default_categories(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    Ok::<_, StatusCode>(Json(mgr.default_categories()))
}

/// GET /signalk/v1/unitpreferences/categories/custom
pub async fn get_custom_categories(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    Ok::<_, StatusCode>(Json(mgr.custom_categories()))
}

/// PUT /signalk/v1/unitpreferences/categories/custom
pub async fn set_custom_categories(
    State(state): State<Arc<ServerState>>,
    Json(cats): Json<CustomCategories>,
) -> impl IntoResponse {
    let mgr = unit_prefs(&state)?;
    mgr.save_custom_categories(&cats)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok::<_, StatusCode>(StatusCode::OK)
}
