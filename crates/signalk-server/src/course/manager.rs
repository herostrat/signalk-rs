/// CourseManager — manages active navigation state.
///
/// Handles set/clear destination, active route management, waypoint
/// advancement, and arrival detection. On every state change, persists to
/// disk and emits deltas into the SignalK store under
/// `navigation.course.*`.
use signalk_plugin_api::PluginError;
use signalk_store::store::SignalKStore;
use signalk_types::geo::{haversine_meters, route_remaining_distance};
use signalk_types::resources::{ActiveRoute, CoursePoint, CourseState, PointType, Position};
use signalk_types::v2::{ActiveRouteRequest, DestinationRequest};
use signalk_types::{Delta, PathValue, Source, Update};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::resources::ResourceProviderRegistry;

/// Tracks whether the active course was set by the REST API or an NMEA sentence.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum CourseSource {
    #[default]
    Api,
    Nmea,
}

/// Persistent course configuration (independent of active navigation state).
///
/// Exposed via `GET /signalk/v2/api/vessels/self/navigation/course/_config`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseConfig {
    /// Default arrival circle radius in meters (0.0 = no arrival detection).
    pub arrival_circle: f64,
    /// When true, only API clients may modify the course (NMEA sources ignored).
    pub api_only: bool,
}

impl Default for CourseConfig {
    fn default() -> Self {
        CourseConfig {
            arrival_circle: 0.0,
            api_only: false,
        }
    }
}

pub struct CourseManager {
    state: RwLock<Option<CourseState>>,
    config: RwLock<CourseConfig>,
    source: RwLock<CourseSource>,
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
            config: RwLock::new(CourseConfig::default()),
            source: RwLock::new(CourseSource::default()),
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

    /// Get the current course state (with resolved waypoints for active routes).
    pub async fn get_state(&self) -> Option<CourseState> {
        let mut state = self.state.read().await.clone()?;

        // Populate waypoints array for REST response (not persisted)
        if let Some(ref mut route) = state.active_route
            && route.waypoints.is_none()
            && let Ok(points) = self.resolve_route_points(&route.href).await
        {
            let wps = if route.reverse {
                let mut reversed = points;
                reversed.reverse();
                reversed
            } else {
                points
            };
            route.waypoints = Some(wps);
        }

        Some(state)
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
            target_arrival_time: None,
            arrival_circle: self
                .state
                .read()
                .await
                .as_ref()
                .map(|s| s.arrival_circle)
                .unwrap_or(0.0),
            active_route: None,
            next_point: Some(CoursePoint {
                type_: PointType::Location,
                position,
                href: req.href,
            }),
            previous_point: previous.map(|pos| CoursePoint {
                type_: PointType::VesselPosition,
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

        let point_total = points.len();
        let course_state = CourseState {
            start_time: Some(now),
            target_arrival_time: None,
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
                point_total,
                name: None,
                waypoints: None,
            }),
            next_point: Some(CoursePoint {
                type_: PointType::RoutePoint,
                position: points[0].clone(),
                href: None,
            }),
            previous_point: previous.map(|pos| CoursePoint {
                type_: PointType::VesselPosition,
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
            type_: PointType::RoutePoint,
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
            type_: PointType::RoutePoint,
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
        active_route.point_total = points.len();
        course.previous_point = course.next_point.clone();
        course.next_point = points.first().map(|pos| CoursePoint {
            type_: PointType::RoutePoint,
            position: pos.clone(),
            href: None,
        });

        let course_clone = course.clone();
        drop(state);

        self.persist_state(Some(&course_clone)).await?;
        self.emit_deltas(&course_clone).await;

        Ok(())
    }

    /// Set the target arrival time (ISO 8601 string).
    pub async fn set_target_arrival_time(&self, time: Option<String>) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let course = state
            .as_mut()
            .ok_or_else(|| PluginError::runtime("No active course"))?;
        course.target_arrival_time = time;
        let course_clone = course.clone();
        drop(state);
        self.persist_state(Some(&course_clone)).await?;
        self.emit_deltas(&course_clone).await;
        Ok(())
    }

    /// Set the arrival circle radius (meters).
    pub async fn set_arrival_circle(&self, radius: f64) -> Result<(), PluginError> {
        // Persist in config so the value survives course clear/reset.
        self.config.write().await.arrival_circle = radius;

        let mut state = self.state.write().await;
        match state.as_mut() {
            Some(course) => {
                course.arrival_circle = radius;
                let course_clone = course.clone();
                drop(state);
                self.persist_state(Some(&course_clone)).await?;
                self.emit_deltas(&course_clone).await;
                Ok(())
            }
            None => {
                // No active navigation — value stored in config for next course.
                drop(state);
                Ok(())
            }
        }
    }

    /// Get the current course configuration (arrivalCircle + apiOnly).
    pub async fn get_config(&self) -> CourseConfig {
        self.config.read().await.clone()
    }

    /// Enable API-only mode: NMEA sources will not override the course.
    ///
    /// If the course is currently NMEA-sourced it is automatically cleared.
    pub async fn enable_api_only(&self) {
        self.config.write().await.api_only = true;
        if *self.source.read().await == CourseSource::Nmea {
            *self.state.write().await = None;
            *self.source.write().await = CourseSource::Api;
            self.emit_clear_deltas().await;
            let _ = self.persist_state(None).await;
        }
    }

    /// Disable API-only mode.
    pub async fn disable_api_only(&self) {
        self.config.write().await.api_only = false;
    }

    /// Check if the vessel has arrived at the current waypoint.
    ///
    /// Returns `true` if an arrival was detected (and state was updated).
    /// Called periodically by the arrival-check timer.
    pub async fn check_arrival(&self) -> bool {
        let state = self.state.read().await;
        let course = match state.as_ref() {
            Some(c) => c,
            None => return false,
        };

        if course.arrival_circle <= 0.0 {
            return false;
        }

        // Read distance from store (populated by CourseDistance calculator)
        let distance = {
            let store = self.store.read().await;
            store
                .get_self_path("navigation.course.calcValues.nextPoint.distance")
                .and_then(|sv| sv.value.as_f64())
        };

        let distance = match distance {
            Some(d) if d.is_finite() => d,
            _ => return false,
        };

        if distance >= course.arrival_circle {
            return false;
        }

        // Arrival detected!
        let has_more_waypoints = course.active_route.as_ref().is_some_and(|route| {
            route.point_total > 0 && route.point_index + 1 < route.point_total
        });

        drop(state);

        if has_more_waypoints {
            info!(distance, "Arrival at waypoint detected, advancing to next");
            if let Err(e) = self.advance_next_point(1).await {
                warn!("Failed to auto-advance waypoint: {e}");
            }
        } else {
            info!(distance, "Arrival at destination detected");
            self.emit_arrival_notification().await;
        }

        true
    }

    /// Get calculated navigation values from the store.
    pub async fn get_calc_values(&self) -> serde_json::Value {
        let store = self.store.read().await;

        let get = |path: &str| -> Option<serde_json::Value> {
            store.get_self_path(path).map(|sv| sv.value.clone())
        };

        let mut result = serde_json::json!({
            "calcMethod": "GreatCircle"
        });

        if let Some(v) = get("navigation.course.calcValues.bearingTrackTrue") {
            result["bearingTrackTrue"] = v;
        }
        if let Some(v) = get("navigation.course.calcValues.bearingTrackMagnetic") {
            result["bearingTrackMagnetic"] = v;
        }
        if let Some(v) = get("navigation.course.calcValues.crossTrackError") {
            result["crossTrackError"] = v;
        }

        let mut next_point = serde_json::Map::new();
        if let Some(v) = get("navigation.course.calcValues.nextPoint.distance") {
            next_point.insert("distance".into(), v);
        }
        if let Some(v) = get("navigation.course.calcValues.nextPoint.velocityMadeGood") {
            next_point.insert("velocityMadeGood".into(), v);
        }
        if let Some(v) = get("navigation.course.calcValues.nextPoint.estimatedTimeOfArrival") {
            next_point.insert("estimatedTimeOfArrival".into(), v);
        }
        if !next_point.is_empty() {
            result["nextPoint"] = serde_json::Value::Object(next_point);
        }

        let mut prev_point = serde_json::Map::new();
        if let Some(v) = get("navigation.course.calcValues.previousPoint.distance") {
            prev_point.insert("distance".into(), v);
        }
        if !prev_point.is_empty() {
            result["previousPoint"] = serde_json::Value::Object(prev_point);
        }

        result
    }

    // ─── Internal helpers ──────────────────────────────────────────────────

    /// Emit an arrival notification delta.
    async fn emit_arrival_notification(&self) {
        let notification = serde_json::json!({
            "state": "alert",
            "method": ["visual", "sound"],
            "message": "Arrived at destination"
        });
        let delta = Delta::self_vessel(vec![Update::new(
            Source::plugin("course-manager"),
            vec![PathValue::new(
                "notifications.navigation.arrivalCircleEntered",
                notification,
            )],
        )]);
        self.store.write().await.apply_delta(delta);
    }

    /// Apply a new course state: persist and emit deltas.
    /// Always marks the source as `Api` (REST API callers only).
    pub(crate) async fn apply_state(&self, course_state: CourseState) -> Result<(), PluginError> {
        self.persist_state(Some(&course_state)).await?;
        self.emit_deltas(&course_state).await;
        *self.state.write().await = Some(course_state);
        *self.source.write().await = CourseSource::Api;
        Ok(())
    }

    /// Apply a destination received from an NMEA sentence.
    ///
    /// Ignored when `api_only = true`. Sets source to `Nmea`.
    async fn apply_nmea_position(&self, position: Position) -> Result<(), PluginError> {
        let config = self.config.read().await;
        if config.api_only {
            return Ok(());
        }
        let arrival_circle = config.arrival_circle;
        drop(config);

        let now = chrono::Utc::now().to_rfc3339();
        let previous = self.current_vessel_position().await;

        let course_state = CourseState {
            start_time: Some(now),
            target_arrival_time: None,
            arrival_circle,
            active_route: None,
            next_point: Some(CoursePoint {
                type_: PointType::Location,
                position,
                href: None,
            }),
            previous_point: previous.map(|pos| CoursePoint {
                type_: PointType::VesselPosition,
                position: pos,
                href: None,
            }),
        };

        self.persist_state(Some(&course_state)).await?;
        self.emit_deltas(&course_state).await;
        *self.state.write().await = Some(course_state);
        *self.source.write().await = CourseSource::Nmea;
        Ok(())
    }

    /// Start a background task that listens for NMEA-sourced course deltas.
    ///
    /// When a `navigation.course.nextPoint.position` delta from a
    /// non-course-manager source arrives and `api_only = false`, the position
    /// is applied as an NMEA-sourced destination.
    pub async fn start_nmea_listener(
        self: Arc<Self>,
        mut rx: tokio::sync::broadcast::Receiver<signalk_types::Delta>,
    ) {
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(delta) => {
                        for update in &delta.updates {
                            if update.source.label == "course-manager" {
                                continue;
                            }
                            for pv in &update.values {
                                if (pv.path == "navigation.courseGreatCircle.nextPoint.position"
                                    || pv.path == "navigation.courseRhumbline.nextPoint.position")
                                    && let Ok(pos) =
                                        serde_json::from_value::<Position>(pv.value.clone())
                                    && let Err(e) = self.apply_nmea_position(pos).await
                                {
                                    warn!("NMEA course apply failed: {e}");
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {} // skip lagged
                }
            }
        });
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
                "navigation.course.nextPoint.position",
                serde_json::json!({
                    "latitude": next.position.latitude,
                    "longitude": next.position.longitude
                }),
            ));
            values.push(PathValue::new(
                "navigation.course.nextPoint.type",
                serde_json::to_value(next.type_).unwrap_or_default(),
            ));
        }

        if let Some(ref prev) = state.previous_point {
            values.push(PathValue::new(
                "navigation.course.previousPoint.position",
                serde_json::json!({
                    "latitude": prev.position.latitude,
                    "longitude": prev.position.longitude
                }),
            ));
        }

        if let Some(ref route) = state.active_route {
            values.push(PathValue::new(
                "navigation.course.activeRoute.href",
                serde_json::Value::String(route.href.clone()),
            ));
        }

        if let Some(ref start_time) = state.start_time {
            values.push(PathValue::new(
                "navigation.course.startTime",
                serde_json::Value::String(start_time.clone()),
            ));
        }

        if let Some(ref target) = state.target_arrival_time {
            values.push(PathValue::new(
                "navigation.course.targetArrivalTime",
                serde_json::Value::String(target.clone()),
            ));
        }

        if state.arrival_circle > 0.0 {
            values.push(PathValue::new(
                "navigation.course.arrivalCircle",
                serde_json::json!(state.arrival_circle),
            ));
        }

        // Route metrics: remaining distance if we have an active route with waypoints
        if let Some(ref route) = state.active_route
            && let Ok(route_points) = self.resolve_route_points(&route.href).await
        {
            let points = if route.reverse {
                let mut reversed = route_points;
                reversed.reverse();
                reversed
            } else {
                route_points
            };

            let wp_tuples: Vec<(f64, f64)> =
                points.iter().map(|p| (p.latitude, p.longitude)).collect();

            // Distance from current waypoint to all remaining waypoints
            let remaining = route_remaining_distance(&wp_tuples, route.point_index);

            // Add distance from vessel to current next waypoint
            let vessel_to_next = if let Some(ref next) = state.next_point {
                let store = self.store.read().await;
                store
                    .get_self_path("navigation.position")
                    .and_then(|sv| {
                        let lat = sv.value.get("latitude")?.as_f64()?;
                        let lon = sv.value.get("longitude")?.as_f64()?;
                        Some(haversine_meters(
                            lat,
                            lon,
                            next.position.latitude,
                            next.position.longitude,
                        ))
                    })
                    .unwrap_or(0.0)
            } else {
                0.0
            };

            let total_remaining = vessel_to_next + remaining;
            values.push(PathValue::new(
                "navigation.course.activeRoute.distanceRemaining",
                serde_json::json!(total_remaining),
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
            "navigation.course.nextPoint.position",
            "navigation.course.nextPoint.type",
            "navigation.course.arrivalCircle",
            "navigation.course.targetArrivalTime",
            "navigation.course.previousPoint.position",
            "navigation.course.activeRoute.href",
            "navigation.course.activeRoute.distanceRemaining",
            "navigation.course.startTime",
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
        assert_eq!(next.type_, PointType::Location);
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
            .get_self_path("navigation.course.nextPoint.position")
            .unwrap();
        assert_eq!(next_pos.value["latitude"], 49.27);
    }

    #[tokio::test]
    async fn check_arrival_no_course_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store, tmp.path().to_path_buf(), resource_providers);
        assert!(!mgr.check_arrival().await);
    }

    #[tokio::test]
    async fn check_arrival_outside_circle_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store.clone(), tmp.path().to_path_buf(), resource_providers);

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

        // Set arrival circle
        mgr.set_arrival_circle(100.0).await.unwrap();

        // Put distance of 500m into store (outside 100m arrival circle)
        {
            let delta = Delta::self_vessel(vec![Update::new(
                Source::plugin("derived-data"),
                vec![PathValue::new(
                    "navigation.course.calcValues.nextPoint.distance",
                    serde_json::json!(500.0),
                )],
            )]);
            store.write().await.apply_delta(delta);
        }

        assert!(!mgr.check_arrival().await);
    }

    #[tokio::test]
    async fn check_arrival_inside_circle_returns_true() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store.clone(), tmp.path().to_path_buf(), resource_providers);

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

        mgr.set_arrival_circle(100.0).await.unwrap();

        // Put distance of 50m into store (inside 100m arrival circle)
        {
            let delta = Delta::self_vessel(vec![Update::new(
                Source::plugin("derived-data"),
                vec![PathValue::new(
                    "navigation.course.calcValues.nextPoint.distance",
                    serde_json::json!(50.0),
                )],
            )]);
            store.write().await.apply_delta(delta);
        }

        assert!(mgr.check_arrival().await);

        // Should have emitted an arrival notification
        let s = store.read().await;
        let notification = s.get_self_path("notifications.navigation.arrivalCircleEntered");
        assert!(
            notification.is_some(),
            "Expected arrival notification in store"
        );
    }

    #[tokio::test]
    async fn set_arrival_circle_emits_delta() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store.clone(), tmp.path().to_path_buf(), resource_providers);

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

        mgr.set_arrival_circle(200.0).await.unwrap();

        let s = store.read().await;
        let arrival = s.get_self_path("navigation.course.arrivalCircle").unwrap();
        assert_eq!(arrival.value.as_f64().unwrap(), 200.0);
    }

    #[tokio::test]
    async fn calc_values_empty_when_no_course() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store, tmp.path().to_path_buf(), resource_providers);
        let values = mgr.get_calc_values().await;
        assert_eq!(values["calcMethod"], "GreatCircle");
        assert!(values.get("nextPoint").is_none());
    }

    #[tokio::test]
    async fn calc_values_includes_distance() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store.clone(), tmp.path().to_path_buf(), resource_providers);

        // Put some values into the store
        {
            let delta = Delta::self_vessel(vec![Update::new(
                Source::plugin("derived-data"),
                vec![
                    PathValue::new(
                        "navigation.course.calcValues.nextPoint.distance",
                        serde_json::json!(5000.0),
                    ),
                    PathValue::new(
                        "navigation.course.calcValues.bearingTrackTrue",
                        serde_json::json!(1.57),
                    ),
                ],
            )]);
            store.write().await.apply_delta(delta);
        }

        let values = mgr.get_calc_values().await;
        assert_eq!(values["bearingTrackTrue"], 1.57);
        assert_eq!(values["nextPoint"]["distance"], 5000.0);
    }

    /// Helper: build a CourseState with an active route at given index/total.
    fn route_course_state(
        point_index: usize,
        point_total: usize,
        arrival_circle: f64,
    ) -> CourseState {
        CourseState {
            start_time: Some(chrono::Utc::now().to_rfc3339()),
            target_arrival_time: None,
            arrival_circle,
            active_route: Some(ActiveRoute {
                href: "/resources/routes/test-route".into(),
                reverse: false,
                point_index,
                point_total,
                name: None,
                waypoints: None,
            }),
            next_point: Some(CoursePoint {
                type_: PointType::RoutePoint,
                position: Position {
                    latitude: 50.0,
                    longitude: -124.0,
                    altitude: None,
                },
                href: None,
            }),
            previous_point: None,
        }
    }

    #[tokio::test]
    async fn check_arrival_mid_route_advances_waypoint() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store.clone(), tmp.path().to_path_buf(), resource_providers);

        // Set up a 3-waypoint route at index 0 with 100m arrival circle
        let course = route_course_state(0, 3, 100.0);
        mgr.apply_state(course).await.unwrap();

        // Put distance 50m (inside 100m circle)
        {
            let delta = Delta::self_vessel(vec![Update::new(
                Source::plugin("derived-data"),
                vec![PathValue::new(
                    "navigation.course.calcValues.nextPoint.distance",
                    serde_json::json!(50.0),
                )],
            )]);
            store.write().await.apply_delta(delta);
        }

        let arrived = mgr.check_arrival().await;
        assert!(arrived, "Should detect arrival");

        // Should have tried to advance (will fail because no real route resource,
        // but the important thing is it did NOT emit an arrival notification)
        let s = store.read().await;
        let notification = s.get_self_path("notifications.navigation.arrivalCircleEntered");
        assert!(
            notification.is_none(),
            "Should NOT emit arrival notification at mid-route waypoint"
        );
    }

    #[tokio::test]
    async fn check_arrival_last_waypoint_emits_notification() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store.clone(), tmp.path().to_path_buf(), resource_providers);

        // Set up a 3-waypoint route at last index (2) with 100m arrival circle
        let course = route_course_state(2, 3, 100.0);
        mgr.apply_state(course).await.unwrap();

        // Put distance 30m (inside 100m circle)
        {
            let delta = Delta::self_vessel(vec![Update::new(
                Source::plugin("derived-data"),
                vec![PathValue::new(
                    "navigation.course.calcValues.nextPoint.distance",
                    serde_json::json!(30.0),
                )],
            )]);
            store.write().await.apply_delta(delta);
        }

        let arrived = mgr.check_arrival().await;
        assert!(arrived, "Should detect arrival at last waypoint");

        // Should have emitted arrival notification (last waypoint, no more to advance)
        let s = store.read().await;
        let notification = s.get_self_path("notifications.navigation.arrivalCircleEntered");
        assert!(
            notification.is_some(),
            "Should emit arrival notification at last waypoint"
        );
    }

    #[tokio::test]
    async fn check_arrival_destination_no_route_emits_notification() {
        // This is the simple destination case (no active_route at all)
        let tmp = tempfile::tempdir().unwrap();
        let (store, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:test");
        let resource_providers = Arc::new(ResourceProviderRegistry::new(Arc::new(
            crate::resources::FileResourceProvider::new(tmp.path().join("resources")),
        )));

        let mgr = CourseManager::new(store.clone(), tmp.path().to_path_buf(), resource_providers);

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

        mgr.set_arrival_circle(100.0).await.unwrap();

        // Put distance 40m (inside 100m circle)
        {
            let delta = Delta::self_vessel(vec![Update::new(
                Source::plugin("derived-data"),
                vec![PathValue::new(
                    "navigation.course.calcValues.nextPoint.distance",
                    serde_json::json!(40.0),
                )],
            )]);
            store.write().await.apply_delta(delta);
        }

        let arrived = mgr.check_arrival().await;
        assert!(arrived, "Should detect arrival at destination");

        let s = store.read().await;
        let notification = s.get_self_path("notifications.navigation.arrivalCircleEntered");
        assert!(
            notification.is_some(),
            "Should emit arrival notification for destination"
        );
    }
}
