/// Features discovery endpoint.
///
/// `GET /signalk/v2/features` returns available APIs and plugins.
use axum::{Json, extract::State, response::IntoResponse};
use signalk_types::{FeatureInfo, FeaturesResponse};
use std::sync::Arc;

use crate::ServerState;

/// GET /signalk/v2/features
///
/// Returns a list of available v2 APIs and registered plugins.
/// Used by webapps (KIP, Freeboard) to discover server capabilities.
pub async fn get_features(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let registry = state.plugin_registry.read().await;

    let plugins: Vec<FeatureInfo> = registry
        .all()
        .iter()
        .map(|p| FeatureInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            enabled: p.enabled,
        })
        .collect();

    Json(FeaturesResponse {
        apis: vec![
            FeatureInfo {
                id: "resources".into(),
                name: "Resources API".into(),
                enabled: true,
            },
            FeatureInfo {
                id: "course".into(),
                name: "Course API".into(),
                enabled: true,
            },
        ],
        plugins,
    })
}
