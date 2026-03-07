//! Admin WebSocket messages: SERVERSTATISTICS and PROVIDERSTATUS.
//!
//! These messages are sent periodically to WebSocket clients that connect with
//! `?serverevents=all`. The Admin UI dispatches them as Redux actions.

use crate::plugins::registry::PluginInfo;
use serde::Serialize;
use std::collections::HashMap;

// ─── SERVERSTATISTICS ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ServerStatisticsMessage {
    pub r#type: &'static str,
    pub data: ServerStatisticsData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatisticsData {
    pub delta_rate: f64,
    pub number_of_available_paths: usize,
    pub ws_clients: usize,
    pub uptime: u64,
    pub provider_statistics: HashMap<String, ProviderStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStats {
    pub delta_rate: f64,
    pub delta_count: u64,
}

impl ServerStatisticsMessage {
    pub fn new(data: ServerStatisticsData) -> Self {
        Self {
            r#type: "SERVERSTATISTICS",
            data,
        }
    }
}

// ─── PROVIDERSTATUS ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ProviderStatusMessage {
    pub r#type: &'static str,
    pub data: Vec<ProviderStatusEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatusEntry {
    pub id: String,
    pub status_type: &'static str,
    pub r#type: &'static str,
    pub message: String,
    pub last_error: String,
    pub last_error_time_stamp: String,
}

impl ProviderStatusMessage {
    pub fn new(data: Vec<ProviderStatusEntry>) -> Self {
        Self {
            r#type: "PROVIDERSTATUS",
            data,
        }
    }
}

/// Build a PROVIDERSTATUS message from the current plugin registry state.
pub fn build_provider_status(plugins: &[PluginInfo]) -> ProviderStatusMessage {
    let entries = plugins
        .iter()
        .map(|p| {
            let status_lower = p.status.to_lowercase();
            let (msg_type, message) = if status_lower.starts_with("running") {
                ("status", p.status.clone())
            } else if status_lower.starts_with("error") {
                ("error", p.status.clone())
            } else {
                ("warning", p.status.clone())
            };

            let last_error = if msg_type == "error" {
                p.status.clone()
            } else {
                String::new()
            };

            let last_error_time_stamp = p
                .last_error_time
                .map(|t| t.to_rfc3339())
                .unwrap_or_default();

            ProviderStatusEntry {
                id: p.id.clone(),
                status_type: "plugin",
                r#type: msg_type,
                message,
                last_error,
                last_error_time_stamp,
            }
        })
        .collect();

    ProviderStatusMessage::new(entries)
}
