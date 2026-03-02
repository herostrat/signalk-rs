/// AutopilotProvider implementation — the handle shared with AutopilotManager.
///
/// `ProviderHandle` is created during `Plugin::start()` and registered with
/// the server's `AutopilotManager`. All V2 API calls (engage, set mode, etc.)
/// delegate here. Thread-safe via `Arc<RwLock<AutopilotState>>`.
use async_trait::async_trait;
use signalk_plugin_api::{
    AutopilotData, AutopilotOptions, AutopilotProvider, PluginError, TackDirection,
};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{
    pd,
    state::{AutopilotMode, AutopilotState},
};

pub(crate) struct ProviderHandle {
    pub(crate) device_id: String,
    pub(crate) state: Arc<RwLock<AutopilotState>>,
}

#[async_trait]
impl AutopilotProvider for ProviderHandle {
    fn device_id(&self) -> &str {
        &self.device_id
    }

    async fn get_data(&self) -> Result<AutopilotData, PluginError> {
        let st = self.state.read().await;
        Ok(AutopilotData {
            state: if st.enabled {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            },
            mode: st.mode.as_str().to_string(),
            target: st.target_rad,
            engaged: st.enabled,
            options: AutopilotOptions {
                modes: {
                    #[cfg(not(feature = "experimental"))]
                    let modes = vec!["compass".to_string(), "wind".to_string()];
                    #[cfg(feature = "experimental")]
                    let modes = vec![
                        "compass".to_string(),
                        "wind".to_string(),
                        "wind_true".to_string(),
                        "route".to_string(),
                    ];
                    modes
                },
            },
        })
    }

    async fn get_state(&self) -> Result<String, PluginError> {
        Ok(if self.state.read().await.enabled {
            "enabled".to_string()
        } else {
            "disabled".to_string()
        })
    }

    async fn set_state(&self, state: &str) -> Result<(), PluginError> {
        match state {
            "enabled" => {
                self.state.write().await.enabled = true;
            }
            "disabled" => {
                let mut st = self.state.write().await;
                st.enabled = false;
                st.last_tick_at = None;
            }
            other => {
                return Err(PluginError::runtime(format!(
                    "unknown autopilot state: {other}"
                )));
            }
        }
        Ok(())
    }

    async fn get_mode(&self) -> Result<String, PluginError> {
        Ok(self.state.read().await.mode.as_str().to_string())
    }

    async fn set_mode(&self, mode: &str) -> Result<(), PluginError> {
        let m: AutopilotMode = mode
            .parse()
            .map_err(|e: String| PluginError::not_found(e))?;
        let mut st = self.state.write().await;
        st.mode = m;
        st.last_error_rad = 0.0;
        st.last_tick_at = None;
        Ok(())
    }

    async fn get_target(&self) -> Result<Option<f64>, PluginError> {
        Ok(self.state.read().await.target_rad)
    }

    async fn set_target(&self, value_rad: f64) -> Result<(), PluginError> {
        self.state.write().await.target_rad = Some(value_rad);
        Ok(())
    }

    async fn adjust_target(&self, delta_rad: f64) -> Result<(), PluginError> {
        let mut st = self.state.write().await;
        let current = st.target_rad.unwrap_or(0.0);
        st.target_rad = Some(pd::normalize_angle(current + delta_rad));
        Ok(())
    }

    async fn engage(&self) -> Result<(), PluginError> {
        self.state.write().await.enabled = true;
        Ok(())
    }

    async fn disengage(&self) -> Result<(), PluginError> {
        let mut st = self.state.write().await;
        st.enabled = false;
        st.last_tick_at = None;
        Ok(())
    }

    async fn tack(&self, direction: TackDirection) -> Result<(), PluginError> {
        let mut st = self.state.write().await;
        let is_wind_mode = st.mode == AutopilotMode::Wind;
        #[cfg(feature = "experimental")]
        let is_wind_mode = is_wind_mode || st.mode == AutopilotMode::WindTrue;
        if !is_wind_mode {
            return Err(PluginError::not_found("tack requires wind mode"));
        }
        let current = st.target_rad.unwrap_or(0.0);
        let magnitude = current.abs().max(0.1);
        let new_target = match direction {
            TackDirection::Port => -magnitude,
            TackDirection::Starboard => magnitude,
        };
        if (new_target - current).abs() < 0.01 {
            return Err(PluginError::runtime("already on that tack"));
        }
        st.target_rad = Some(new_target);
        st.last_error_rad = 0.0; // reset D-term after maneuver
        Ok(())
    }

    async fn gybe(&self, direction: TackDirection) -> Result<(), PluginError> {
        let mut st = self.state.write().await;
        let is_wind_mode = st.mode == AutopilotMode::Wind;
        #[cfg(feature = "experimental")]
        let is_wind_mode = is_wind_mode || st.mode == AutopilotMode::WindTrue;
        if !is_wind_mode {
            return Err(PluginError::not_found("gybe requires wind mode"));
        }
        // Gybe: rotate target through dead downwind (±180°)
        let current = st.target_rad.unwrap_or(0.0);
        let magnitude = current.abs().max(0.1);
        // Running downwind means large magnitude (~150–180°); gybe flips side
        let new_target = match direction {
            TackDirection::Port => -magnitude,
            TackDirection::Starboard => magnitude,
        };
        if (new_target - current).abs() < 0.01 {
            return Err(PluginError::runtime("already on that gybe"));
        }
        st.target_rad = Some(new_target);
        st.last_error_rad = 0.0;
        Ok(())
    }

    async fn dodge(&self, offset_rad: Option<f64>) -> Result<(), PluginError> {
        self.state.write().await.dodge_offset_rad = offset_rad;
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::state::AutopilotState;

    pub(crate) fn make_provider(state: AutopilotState) -> ProviderHandle {
        ProviderHandle {
            device_id: "default".to_string(),
            state: Arc::new(RwLock::new(state)),
        }
    }

    pub(crate) fn compass_state() -> AutopilotState {
        let mut st = AutopilotState::new(AutopilotMode::Compass);
        st.enabled = true;
        st.target_rad = Some(1.0);
        st
    }

    pub(crate) fn wind_state() -> AutopilotState {
        let mut st = AutopilotState::new(AutopilotMode::Wind);
        st.enabled = true;
        st.target_rad = Some(0.7); // ~40° starboard tack
        st
    }

    // ── State / engage / disengage ─────────────────────────────────────────────

    #[tokio::test]
    async fn get_state_enabled() {
        let p = make_provider(compass_state());
        assert_eq!(p.get_state().await.unwrap(), "enabled");
    }

    #[tokio::test]
    async fn get_state_disabled() {
        let mut st = compass_state();
        st.enabled = false;
        let p = make_provider(st);
        assert_eq!(p.get_state().await.unwrap(), "disabled");
    }

    #[tokio::test]
    async fn set_state_enables() {
        let mut st = compass_state();
        st.enabled = false;
        let p = make_provider(st);
        p.set_state("enabled").await.unwrap();
        assert!(p.state.read().await.enabled);
    }

    #[tokio::test]
    async fn set_state_disables() {
        let p = make_provider(compass_state());
        p.set_state("disabled").await.unwrap();
        assert!(!p.state.read().await.enabled);
    }

    #[tokio::test]
    async fn set_state_unknown_returns_error() {
        let p = make_provider(compass_state());
        assert!(p.set_state("sailing").await.is_err());
    }

    #[tokio::test]
    async fn engage_sets_enabled() {
        let mut st = compass_state();
        st.enabled = false;
        let p = make_provider(st);
        p.engage().await.unwrap();
        assert!(p.state.read().await.enabled);
    }

    #[tokio::test]
    async fn disengage_clears_enabled_and_tick() {
        let p = make_provider(compass_state());
        p.disengage().await.unwrap();
        let st = p.state.read().await;
        assert!(!st.enabled);
        assert!(st.last_tick_at.is_none());
    }

    // ── Mode ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_mode_compass() {
        let p = make_provider(compass_state());
        assert_eq!(p.get_mode().await.unwrap(), "compass");
    }

    #[tokio::test]
    async fn set_mode_wind() {
        let p = make_provider(compass_state());
        p.set_mode("wind").await.unwrap();
        assert_eq!(p.state.read().await.mode, AutopilotMode::Wind);
    }

    #[tokio::test]
    async fn set_mode_resets_d_term() {
        let mut st = compass_state();
        st.last_error_rad = 0.5;
        let p = make_provider(st);
        p.set_mode("wind").await.unwrap();
        assert_eq!(p.state.read().await.last_error_rad, 0.0);
    }

    #[tokio::test]
    async fn set_mode_unknown_returns_not_found() {
        let p = make_provider(compass_state());
        assert!(p.set_mode("magic").await.unwrap_err().is_not_found());
    }

    // ── Target ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_target_returns_value() {
        let p = make_provider(compass_state()); // target = 1.0
        assert!((p.get_target().await.unwrap().unwrap() - 1.0).abs() < 1e-10);
    }

    #[tokio::test]
    async fn set_target_updates_value() {
        let p = make_provider(compass_state());
        p.set_target(std::f64::consts::PI).await.unwrap();
        assert!((p.state.read().await.target_rad.unwrap() - std::f64::consts::PI).abs() < 1e-10);
    }

    #[tokio::test]
    async fn adjust_target_adds_delta() {
        let p = make_provider(compass_state()); // target = 1.0
        p.adjust_target(0.1).await.unwrap();
        assert!((p.state.read().await.target_rad.unwrap() - 1.1).abs() < 1e-10);
    }

    #[tokio::test]
    async fn adjust_target_wraps_at_pi() {
        let mut st = compass_state();
        st.target_rad = Some(std::f64::consts::PI - 0.05);
        let p = make_provider(st);
        p.adjust_target(0.1).await.unwrap();
        // Should wrap to negative side
        assert!(p.state.read().await.target_rad.unwrap() < 0.0);
    }

    // ── Tack ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn tack_requires_wind_mode() {
        let p = make_provider(compass_state());
        assert!(
            p.tack(TackDirection::Port)
                .await
                .unwrap_err()
                .is_not_found()
        );
    }

    #[tokio::test]
    async fn tack_port_sets_negative_target() {
        let p = make_provider(wind_state()); // target = +0.7 (starboard)
        p.tack(TackDirection::Port).await.unwrap();
        assert!(p.state.read().await.target_rad.unwrap() < 0.0);
    }

    #[tokio::test]
    async fn tack_starboard_sets_positive_target() {
        let mut st = wind_state();
        st.target_rad = Some(-0.7); // port tack
        let p = make_provider(st);
        p.tack(TackDirection::Starboard).await.unwrap();
        assert!(p.state.read().await.target_rad.unwrap() > 0.0);
    }

    #[tokio::test]
    async fn tack_already_on_that_tack_returns_error() {
        let p = make_provider(wind_state()); // positive = starboard
        assert!(p.tack(TackDirection::Starboard).await.is_err());
    }

    #[tokio::test]
    async fn tack_resets_d_term() {
        let mut st = wind_state();
        st.last_error_rad = 0.5;
        let p = make_provider(st);
        p.tack(TackDirection::Port).await.unwrap();
        assert_eq!(p.state.read().await.last_error_rad, 0.0);
    }

    // ── Gybe ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn gybe_requires_wind_mode() {
        let p = make_provider(compass_state());
        assert!(
            p.gybe(TackDirection::Port)
                .await
                .unwrap_err()
                .is_not_found()
        );
    }

    // ── Dodge ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dodge_sets_offset() {
        let p = make_provider(compass_state());
        p.dodge(Some(0.1)).await.unwrap();
        assert!((p.state.read().await.dodge_offset_rad.unwrap() - 0.1).abs() < 1e-10);
    }

    #[tokio::test]
    async fn dodge_none_clears_offset() {
        let p = make_provider(compass_state());
        p.dodge(Some(0.1)).await.unwrap();
        p.dodge(None).await.unwrap();
        assert!(p.state.read().await.dodge_offset_rad.is_none());
    }

    // ── get_data ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_data_includes_stable_modes() {
        let p = make_provider(compass_state());
        let data = p.get_data().await.unwrap();
        assert!(data.options.modes.contains(&"compass".to_string()));
        assert!(data.options.modes.contains(&"wind".to_string()));
    }

    #[cfg(feature = "experimental")]
    #[tokio::test]
    async fn get_data_includes_experimental_route_mode() {
        let p = make_provider(compass_state());
        let data = p.get_data().await.unwrap();
        assert!(data.options.modes.contains(&"route".to_string()));
    }

    #[tokio::test]
    async fn get_data_engaged_when_enabled() {
        let p = make_provider(compass_state());
        assert!(p.get_data().await.unwrap().engaged);
    }
}
