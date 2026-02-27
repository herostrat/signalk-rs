/// CourseManager — manages active navigation state.
///
/// Handles set/clear destination, active route management, and waypoint
/// advancement. On every state change, persists to disk and emits deltas
/// into the SignalK store under `navigation.courseGreatCircle.*`.
use signalk_plugin_api::PluginError;
use signalk_store::store::SignalKStore;
use signalk_types::resources::{ActiveRoute, CoursePoint, CourseState, PointType, Position};
use signalk_types::v2::{ActiveRouteRequest, DestinationRequest};
use signalk_types::{Delta, PathValue, Source, Update};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::resources::ResourceProviderRegistry;

pub struct CourseManager {
    state: RwLock<Option<CourseState>>,
    store: Arc<RwLock<SignalKStore>>,
    state_file: PathBuf,
    resource_providers: Arc<ResourceProviderRegistry>,
}

impl CourseManager {
    pub fn new(
        store: Arc<RwLock<SignalKStore>>,
        data_dir: PathBuf,
        resource_providers: Arc<ResourceProviderRegistry>,
    ) -> Self {
        CourseManager {
            state: RwLock::new(None),
            store,
            state_file: data_dir.join("course").join("state.json"),
            resource_providers,
        }
    }

    /// Load persisted course state from disk (if any).
    pub async fn load(&self) {
        match tokio::fs::read_to_string(&self.state_file).await {
            Ok(contents) => match serde_json::from_str::<CourseState>(&contents) {
                Ok(state) => {
                    info!("Restored course state from disk");
                    self.emit_deltas(&state).await;
                    *self.state.write().await = Some(state);
                }
                Err(e) => {
                    debug!("Could not parse course state file: {e}");
                }
            },
            Err(_) => {
                debug!("No persisted course state found");
            }
        }
    }

    /// Get the current course state.
    pub async fn get_state(&self) -> Option<CourseState> {
        self.state.read().await.clone()
    }

    /// Set a direct destination (position or waypoint href).
    pub async fn set_destination(&self, req: DestinationRequest) -> Result<(), PluginError> {
        let position = match (req.position, req.href.as_deref()) {
            (Some(pos), _) => pos,
            (None, Some(href)) => self.resolve_waypoint_position(href).await?,
            (None, None) => {
                return Err(PluginError::config(
                    "Either position or href must be provided",
                ));
            }
        };

        let now = chrono::Utc::now().to_rfc3339();
        let previous = self.current_vessel_position().await;

        let course_state = CourseState {
            start_time: Some(now),
            arrival_circle: self
                .state
                .read()
                .await
                .as_ref()
                .map(|s| s.arrival_circle)
                .unwrap_or(0.0),
            active_route: None,
            next_point: Some(CoursePoint {
                type_: PointType::Destination,
                position,
                href: req.href,
            }),
            previous_point: previous.map(|pos| CoursePoint {
                type_: PointType::Destination,
                position: pos,
                href: None,
            }),
        };

        self.apply_state(course_state).await
    }

    /// Set an active route to follow.
    pub async fn set_active_route(&self, req: ActiveRouteRequest) -> Result<(), PluginError> {
        let route_points = self.resolve_route_points(&req.href).await?;

        if route_points.is_empty() {
            return Err(PluginError::runtime("Route has no waypoints"));
        }

        let (point_index, points) = if req.reverse {
            let mut reversed = route_points;
            reversed.reverse();
            (0, reversed)
        } else {
            (0, route_points)
        };

        let now = chrono::Utc::now().to_rfc3339();
        let previous = self.current_vessel_position().await;

        let course_state = CourseState {
            start_time: Some(now),
            arrival_circle: self
                .state
                .read()
                .await
                .as_ref()
                .map(|s| s.arrival_circle)
                .unwrap_or(0.0),
            active_route: Some(ActiveRoute {
                href: req.href,
                reverse: req.reverse,
                point_index,
                name: None,
            }),
            next_point: Some(CoursePoint {
                type_: PointType::Waypoint,
                position: points[0].clone(),
                href: None,
            }),
            previous_point: previous.map(|pos| CoursePoint {
                type_: PointType::Destination,
                position: pos,
                href: None,
            }),
        };

        self.apply_state(course_state).await
    }

    /// Clear the course (stop navigation).
    pub async fn clear(&self) -> Result<(), PluginError> {
        *self.state.write().await = None;
        self.persist_state(None).await?;
        self.emit_clear_deltas().await;
        info!("Course cleared");
        Ok(())
    }

    /// Advance to the next (or previous) waypoint in the active route.
    pub async fn advance_next_point(&self, delta: i32) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let course = state
            .as_mut()
            .ok_or_else(|| PluginError::runtime("No active course"))?;
        let active_route = course
            .active_route
            .as_mut()
            .ok_or_else(|| PluginError::runtime("No active route"))?;

        let route_points = self.resolve_route_points(&active_route.href).await?;

        let new_index = active_route.point_index as i64 + delta as i64;
        if new_index < 0 || new_index as usize >= route_points.len() {
            return Err(PluginError::runtime(format!(
                "Point index {new_index} out of range (0..{})",
                route_points.len()
            )));
        }

        let new_index = new_index as usize;

        // Previous point becomes current next point
        course.previous_point = course.next_point.clone();

        let points = if active_route.reverse {
            let mut reversed = route_points;
            reversed.reverse();
            reversed
        } else {
            route_points
        };

        active_route.point_index = new_index;
        course.next_point = Some(CoursePoint {
            type_: PointType::Waypoint,
            position: points[new_index].clone(),
            href: None,
        });

        let course_clone = course.clone();
        drop(state);

        self.persist_state(Some(&course_clone)).await?;
        self.emit_deltas(&course_clone).await;

        Ok(())
    }

    /// Jump to a specific waypoint index in the active route.
    pub async fn set_point_index(&self, index: usize) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let course = state
            .as_mut()
            .ok_or_else(|| PluginError::runtime("No active course"))?;
        let active_route = course
            .active_route
            .as_mut()
            .ok_or_else(|| PluginError::runtime("No active route"))?;

        let route_points = self.resolve_route_points(&active_route.href).await?;

        if index >= route_points.len() {
            return Err(PluginError::runtime(format!(
                "Point index {index} out of range (0..{})",
                route_points.len()
            )));
        }

        let points = if active_route.reverse {
            let mut reversed = route_points;
            reversed.reverse();
            reversed
        } else {
            route_points
        };

        course.previous_point = course.next_point.clone();
        active_route.point_index = index;
        course.next_point = Some(CoursePoint {
            type_: PointType::Waypoint,
            position: points[index].clone(),
            href: None,
        });

        let course_clone = course.clone();
        drop(state);

        self.persist_state(Some(&course_clone)).await?;
        self.emit_deltas(&course_clone).await;

        Ok(())
    }

    /// Reverse the direction of the active route.
    pub async fn reverse_route(&self) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let course = state
            .as_mut()
            .ok_or_else(|| PluginError::runtime("No active course"))?;
        let active_route = course
            .active_route
            .as_mut()
            .ok_or_else(|| PluginError::runtime("No active route"))?;

        active_route.reverse = !active_route.reverse;

        let route_points = self.resolve_route_points(&active_route.href).await?;

        let points = if active_route.reverse {
            let mut reversed = route_points;
            reversed.reverse();
            reversed
        } else {
            route_points
        };

        // Reset to first point after reversal
        active_route.point_index = 0;
        course.previous_point = course.next_point.clone();
        course.next_point = points.first().map(|pos| CoursePoint {
            type_: PointType::Waypoint,
            position: pos.clone(),
            href: None,
        });

        let course_clone = course.clone();
        drop(state);

        self.persist_state(Some(&course_clone)).await?;
        self.emit_deltas(&course_clone).await;

        Ok(())
    }

    // ─── Internal helpers ──────────────────────────────────────────────────

    /// Apply a new course state: persist and emit deltas.
    async fn apply_state(&self, course_state: CourseState) -> Result<(), PluginError> {
        self.persist_state(Some(&course_state)).await?;
        self.emit_deltas(&course_state).await;
        *self.state.write().await = Some(course_state);
        Ok(())
    }

    /// Persist course state to disk.
    async fn persist_state(&self, state: Option<&CourseState>) -> Result<(), PluginError> {
        if let Some(parent) = self.state_file.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| PluginError::runtime(format!("mkdir course dir: {e}")))?;
        }

        match state {
            Some(s) => {
                let json = serde_json::to_string_pretty(s)
                    .map_err(|e| PluginError::runtime(format!("serialize course: {e}")))?;
                tokio::fs::write(&self.state_file, json)
                    .await
                    .map_err(|e| PluginError::runtime(format!("write course state: {e}")))?;
            }
            None => {
                let _ = tokio::fs::remove_file(&self.state_file).await;
            }
        }

        Ok(())
    }

    /// Emit course state as SignalK deltas into the store.
    async fn emit_deltas(&self, state: &CourseState) {
        let mut values = Vec::new();

        if let Some(ref next) = state.next_point {
            values.push(PathValue::new(
                "navigation.courseGreatCircle.nextPoint.position",
                serde_json::json!({
                    "latitude": next.position.latitude,
                    "longitude": next.position.longitude
                }),
            ));
            values.push(PathValue::new(
                "navigation.courseGreatCircle.nextPoint.type",
                serde_json::to_value(next.type_).unwrap_or_default(),
            ));
        }

        if let Some(ref prev) = state.previous_point {
            values.push(PathValue::new(
                "navigation.courseGreatCircle.previousPoint.position",
                serde_json::json!({
                    "latitude": prev.position.latitude,
                    "longitude": prev.position.longitude
                }),
            ));
        }

        if let Some(ref route) = state.active_route {
            values.push(PathValue::new(
                "navigation.courseGreatCircle.activeRoute.href",
                serde_json::Value::String(route.href.clone()),
            ));
        }

        if let Some(ref start_time) = state.start_time {
            values.push(PathValue::new(
                "navigation.courseGreatCircle.startTime",
                serde_json::Value::String(start_time.clone()),
            ));
        }

        if !values.is_empty() {
            let delta =
                Delta::self_vessel(vec![Update::new(Source::plugin("course-manager"), values)]);
            self.store.write().await.apply_delta(delta);
        }
    }

    /// Emit null values for all course paths (on clear).
    async fn emit_clear_deltas(&self) {
        let paths = [
            "navigation.courseGreatCircle.nextPoint.position",
            "navigation.courseGreatCircle.nextPoint.type",
            "navigation.courseGreatCircle.previousPoint.position",
            "navigation.courseGreatCircle.activeRoute.href",
            "navigation.courseGreatCircle.startTime",
        ];

        let values: Vec<PathValue> = paths
            .iter()
            .map(|p| PathValue::new(*p, serde_json::Value::Null))
            .collect();

        let delta = Delta::self_vessel(vec![Update::new(Source::plugin("course-manager"), values)]);
        self.store.write().await.apply_delta(delta);
    }

    /// Get the vessel's current position from the store.
    async fn current_vessel_position(&self) -> Option<Position> {
        let store = self.store.read().await;
        let value = store.get_self_path("navigation.position")?;
        let lat = value.value.get("latitude")?.as_f64()?;
        let lon = value.value.get("longitude")?.as_f64()?;
        Some(Position {
            latitude: lat,
            longitude: lon,
            altitude: None,
        })
    }

    /// Resolve a waypoint href to a Position.
    ///
    /// Expects href like `/resources/waypoints/{id}`.
    async fn resolve_waypoint_position(&self, href: &str) -> Result<Position, PluginError> {
        let (resource_type, id) = parse_resource_href(href)?;
        let value = self
            .resource_providers
            .get(&resource_type, &id)
            .await?
            .ok_or_else(|| PluginError::runtime(format!("Waypoint not found: {href}")))?;

        extract_position(&value)
    }

    /// Resolve a route href to a list of positions (waypoints along the route).
    ///
    /// Expects href like `/resources/routes/{id}`.
    /// The route should have a `points` field with `coordinates` (GeoJSON LineString).
    async fn resolve_route_points(&self, href: &str) -> Result<Vec<Position>, PluginError> {
        let (resource_type, id) = parse_resource_href(href)?;
        let value = self
            .resource_providers
            .get(&resource_type, &id)
            .await?
            .ok_or_else(|| PluginError::runtime(format!("Route not found: {href}")))?;

        // Try GeoJSON Feature format: feature.geometry.coordinates
        if let Some(arr) = value
            .pointer("/feature/geometry/coordinates")
            .or_else(|| value.pointer("/geometry/coordinates"))
            .or_else(|| value.get("coordinates"))
            .and_then(|c| c.as_array())
        {
            let positions: Vec<Position> = arr
                .iter()
                .filter_map(|coord| {
                    let lon = coord.get(0)?.as_f64()?;
                    let lat = coord.get(1)?.as_f64()?;
                    Some(Position {
                        latitude: lat,
                        longitude: lon,
                        altitude: None,
                    })
                })
                .collect();

            if !positions.is_empty() {
                return Ok(positions);
            }
        }

        // Try waypoints array format
        if let Some(waypoints) = value.get("waypoints").and_then(|w| w.as_array()) {
            let positions: Vec<Position> = waypoints
                .iter()
                .filter_map(|wp| extract_position(wp).ok())
                .collect();

            if !positions.is_empty() {
                return Ok(positions);
            }
        }

        Err(PluginError::runtime(format!(
            "Could not extract route points from {href}"
        )))
    }
}

/// Parse a resource href like `/resources/waypoints/abc-123` into `("waypoints", "abc-123")`.
fn parse_resource_href(href: &str) -> Result<(String, String), PluginError> {
    let parts: Vec<&str> = href.trim_start_matches('/').split('/').collect();
    if parts.len() >= 3 && parts[0] == "resources" {
        Ok((parts[1].to_string(), parts[2..].join("/")))
    } else {
        Err(PluginError::config(format!(
            "Invalid resource href: {href}"
        )))
    }
}

/// Extract a Position from a resource value.
fn extract_position(value: &serde_json::Value) -> Result<Position, PluginError> {
    // Try direct position field
    if let Some((lat, lon)) = value.get("position").and_then(|pos| {
        let lat = pos.get("latitude")?.as_f64()?;
        let lon = pos.get("longitude")?.as_f64()?;
        Some((lat, lon))
    }) {
        return Ok(Position {
            latitude: lat,
            longitude: lon,
            altitude: value
                .get("position")
                .and_then(|p| p.get("altitude"))
                .and_then(|v| v.as_f64()),
        });
    }

    // Try GeoJSON Feature format: feature.geometry.coordinates [lon, lat]
    if let Some((lon, lat)) = value
        .pointer("/feature/geometry/coordinates")
        .or_else(|| value.pointer("/geometry/coordinates"))
        .and_then(|coords| {
            let lon = coords.get(0)?.as_f64()?;
            let lat = coords.get(1)?.as_f64()?;
            Some((lon, lat))
        })
    {
        return Ok(Position {
            latitude: lat,
            longitude: lon,
            altitude: None,
        });
    }

    Err(PluginError::runtime(
        "Could not extract position from resource",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_href_valid() {
        let (t, id) = parse_resource_href("/resources/waypoints/abc-123").unwrap();
        assert_eq!(t, "waypoints");
        assert_eq!(id, "abc-123");
    }

    #[test]
    fn parse_href_no_leading_slash() {
        let (t, id) = parse_resource_href("resources/routes/xyz").unwrap();
        assert_eq!(t, "routes");
        assert_eq!(id, "xyz");
    }

    #[test]
    fn parse_href_invalid() {
        assert!(parse_resource_href("/invalid").is_err());
        assert!(parse_resource_href("").is_err());
    }

    #[test]
    fn extract_position_direct() {
        let val = serde_json::json!({
            "position": { "latitude": 49.27, "longitude": -123.19 }
        });
        let pos = extract_position(&val).unwrap();
        assert_eq!(pos.latitude, 49.27);
        assert_eq!(pos.longitude, -123.19);
    }

    #[test]
    fn extract_position_geojson() {
        let val = serde_json::json!({
            "feature": {
                "geometry": {
                    "type": "Point",
                    "coordinates": [-123.19, 49.27]
                }
            }
        });
        let pos = extract_position(&val).unwrap();
        assert_eq!(pos.latitude, 49.27);
        assert_eq!(pos.longitude, -123.19);
    }

    #[test]
    fn extract_position_missing() {
        assert!(extract_position(&serde_json::json!({})).is_err());
    }

    #[tokio::test]
    async fn course_manager_set_and_get_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store, tmp.path().to_path_buf(), resource_providers);

        assert!(mgr.get_state().await.is_none());

        mgr.set_destination(DestinationRequest {
            position: Some(Position {
                latitude: 49.27,
                longitude: -123.19,
                altitude: None,
            }),
            href: None,
        })
        .await
        .unwrap();

        let state = mgr.get_state().await.unwrap();
        assert!(state.start_time.is_some());
        let next = state.next_point.unwrap();
        assert_eq!(next.type_, PointType::Destination);
        assert_eq!(next.position.latitude, 49.27);
    }

    #[tokio::test]
    async fn course_manager_clear() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store, tmp.path().to_path_buf(), resource_providers);

        mgr.set_destination(DestinationRequest {
            position: Some(Position {
                latitude: 49.0,
                longitude: -123.0,
                altitude: None,
            }),
            href: None,
        })
        .await
        .unwrap();

        assert!(mgr.get_state().await.is_some());

        mgr.clear().await.unwrap();
        assert!(mgr.get_state().await.is_none());
    }

    #[tokio::test]
    async fn course_manager_persists_and_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        // Set a destination
        let mgr = CourseManager::new(
            store.clone(),
            tmp.path().to_path_buf(),
            resource_providers.clone(),
        );
        mgr.set_destination(DestinationRequest {
            position: Some(Position {
                latitude: 50.0,
                longitude: -124.0,
                altitude: None,
            }),
            href: None,
        })
        .await
        .unwrap();

        // Create a new manager and load from disk
        let mgr2 = CourseManager::new(store, tmp.path().to_path_buf(), resource_providers);
        assert!(mgr2.get_state().await.is_none());

        mgr2.load().await;
        let state = mgr2.get_state().await.unwrap();
        assert_eq!(state.next_point.unwrap().position.latitude, 50.0);
    }

    #[tokio::test]
    async fn course_manager_emits_deltas() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store.clone(), tmp.path().to_path_buf(), resource_providers);

        mgr.set_destination(DestinationRequest {
            position: Some(Position {
                latitude: 49.27,
                longitude: -123.19,
                altitude: None,
            }),
            href: None,
        })
        .await
        .unwrap();

        // Check that the store has the course data
        let s = store.read().await;
        let next_pos = s
            .get_self_path("navigation.courseGreatCircle.nextPoint.position")
            .unwrap();
        assert_eq!(next_pos.value["latitude"], 49.27);
    }
}
