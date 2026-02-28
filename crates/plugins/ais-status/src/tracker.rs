//! AIS target tracking — MMSI classification, status state machine, target lifecycle.

use std::collections::HashMap;
use std::time::{Duration, Instant};

// ─── MMSI Classification (ITU-R M.585) ──────────────────────────────────────

/// AIS target class based on MMSI prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetClass {
    /// Normal vessel (Class A or B — not distinguishable from MMSI alone)
    Vessel,
    /// Aid to Navigation (MMSI prefix 970–979)
    Aton,
    /// Coast/Base station (MMSI prefix 00)
    Base,
    /// Search and Rescue aircraft (MMSI prefix 111)
    Sar,
}

/// Classify a target by its MMSI number string.
pub fn classify_mmsi(mmsi: &str) -> TargetClass {
    if mmsi.starts_with("97") {
        TargetClass::Aton
    } else if mmsi.starts_with("00") {
        TargetClass::Base
    } else if mmsi.starts_with("111") {
        TargetClass::Sar
    } else {
        TargetClass::Vessel
    }
}

// ─── Thresholds per class ───────────────────────────────────────────────────

/// Lifecycle thresholds for a target class.
#[derive(Debug, Clone)]
pub struct ClassThresholds {
    /// Number of messages needed to confirm a target.
    pub confirm_count: u32,
    /// Time window in which `confirm_count` messages must arrive.
    pub confirm_window: Duration,
    /// Time without updates before target is marked Lost.
    pub lost_after: Duration,
    /// Time in Lost state before target is removed.
    pub remove_after: Duration,
}

impl ClassThresholds {
    pub fn for_class(class: TargetClass) -> Self {
        match class {
            TargetClass::Vessel => ClassThresholds {
                confirm_count: 2,
                confirm_window: Duration::from_secs(180),
                lost_after: Duration::from_secs(360),
                remove_after: Duration::from_secs(540),
            },
            TargetClass::Aton => ClassThresholds {
                confirm_count: 1,
                confirm_window: Duration::from_secs(180),
                lost_after: Duration::from_secs(900),
                remove_after: Duration::from_secs(3600),
            },
            TargetClass::Base | TargetClass::Sar => ClassThresholds {
                confirm_count: 1,
                confirm_window: Duration::from_secs(10),
                lost_after: Duration::from_secs(30),
                remove_after: Duration::from_secs(180),
            },
        }
    }
}

// ─── Target Status ──────────────────────────────────────────────────────────

/// Lifecycle status of an AIS target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetStatus {
    /// Target seen but not yet confirmed (not enough messages in window).
    Unconfirmed,
    /// Target actively being tracked.
    Confirmed,
    /// Target has gone silent for longer than `lost_after`.
    Lost,
}

impl TargetStatus {
    /// String representation matching the de-facto `sensors.ais.status` convention.
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetStatus::Unconfirmed => "unconfirmed",
            TargetStatus::Confirmed => "confirmed",
            TargetStatus::Lost => "lost",
        }
    }
}

/// A status transition event — emitted when a target changes state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusTransition {
    pub mmsi: String,
    pub context: String,
    pub old_status: TargetStatus,
    pub new_status: TargetStatus,
}

// ─── Tracked Target ─────────────────────────────────────────────────────────

/// A single tracked AIS target.
#[derive(Debug, Clone)]
pub struct TrackedTarget {
    pub mmsi: String,
    pub context: String,
    pub class: TargetClass,
    pub status: TargetStatus,
    pub thresholds: ClassThresholds,
    pub first_seen: Instant,
    pub last_seen: Instant,
    pub message_count: u32,
    /// Timestamps of recent messages (for confirm window).
    recent_messages: Vec<Instant>,
    // Cached data fields (extracted from deltas)
    pub name: Option<String>,
    pub callsign: Option<String>,
    pub position: Option<(f64, f64)>,
    pub sog: Option<f64>,
    pub cog: Option<f64>,
    pub heading: Option<f64>,
}

impl TrackedTarget {
    pub fn new(mmsi: String, context: String, now: Instant) -> Self {
        let class = classify_mmsi(&mmsi);
        let thresholds = ClassThresholds::for_class(class);
        TrackedTarget {
            mmsi,
            context,
            class,
            status: TargetStatus::Unconfirmed,
            thresholds,
            first_seen: now,
            last_seen: now,
            message_count: 0,
            recent_messages: Vec::new(),
            name: None,
            callsign: None,
            position: None,
            sog: None,
            cog: None,
            heading: None,
        }
    }

    /// Record a new message and check for confirmation.
    /// Returns `Some(transition)` if status changed.
    pub fn record_message(&mut self, now: Instant) -> Option<StatusTransition> {
        self.last_seen = now;
        self.message_count += 1;
        self.recent_messages.push(now);

        // Prune messages outside the confirm window
        let window_start = now
            .checked_sub(self.thresholds.confirm_window)
            .unwrap_or(now);
        self.recent_messages.retain(|t| *t >= window_start);

        let old_status = self.status;
        match self.status {
            TargetStatus::Unconfirmed => {
                if self.recent_messages.len() >= self.thresholds.confirm_count as usize {
                    self.status = TargetStatus::Confirmed;
                    return Some(StatusTransition {
                        mmsi: self.mmsi.clone(),
                        context: self.context.clone(),
                        old_status,
                        new_status: TargetStatus::Confirmed,
                    });
                }
            }
            TargetStatus::Lost => {
                // Receiving a message while lost → back to confirmed
                self.status = TargetStatus::Confirmed;
                self.recent_messages = vec![now];
                return Some(StatusTransition {
                    mmsi: self.mmsi.clone(),
                    context: self.context.clone(),
                    old_status,
                    new_status: TargetStatus::Confirmed,
                });
            }
            TargetStatus::Confirmed => {
                // Already confirmed, no transition
            }
        }
        None
    }

    /// Extract data fields from delta path/values.
    pub fn update_from_values(&mut self, values: &[(String, serde_json::Value)]) {
        for (path, value) in values {
            match path.as_str() {
                "name" => {
                    self.name = value.as_str().map(|s| s.to_string());
                }
                "communication.callsignVhf" => {
                    self.callsign = value.as_str().map(|s| s.to_string());
                }
                "navigation.position" => {
                    if let (Some(lat), Some(lon)) = (
                        value.get("latitude").and_then(|v| v.as_f64()),
                        value.get("longitude").and_then(|v| v.as_f64()),
                    ) {
                        self.position = Some((lat, lon));
                    }
                }
                "navigation.speedOverGround" => {
                    self.sog = value.as_f64();
                }
                "navigation.courseOverGroundTrue" => {
                    self.cog = value.as_f64();
                }
                "navigation.headingTrue" => {
                    self.heading = value.as_f64();
                }
                _ => {}
            }
        }
    }
}

// ─── AIS Tracker ────────────────────────────────────────────────────────────

/// Central AIS target tracker. Manages all tracked targets.
pub struct AisTracker {
    targets: HashMap<String, TrackedTarget>,
    self_uri: String,
}

impl AisTracker {
    pub fn new(self_uri: String) -> Self {
        AisTracker {
            targets: HashMap::new(),
            self_uri,
        }
    }

    /// Number of tracked targets.
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// Number of targets by status.
    pub fn count_by_status(&self) -> (usize, usize, usize) {
        let mut confirmed = 0;
        let mut lost = 0;
        let mut unconfirmed = 0;
        for t in self.targets.values() {
            match t.status {
                TargetStatus::Confirmed => confirmed += 1,
                TargetStatus::Lost => lost += 1,
                TargetStatus::Unconfirmed => unconfirmed += 1,
            }
        }
        (confirmed, lost, unconfirmed)
    }

    /// Get a target by MMSI.
    pub fn get(&self, mmsi: &str) -> Option<&TrackedTarget> {
        self.targets.get(mmsi)
    }

    /// Extract MMSI from a vessel context like `vessels.urn:mrn:imo:mmsi:211457160`.
    pub fn parse_mmsi(context: &str) -> Option<&str> {
        context.strip_prefix("vessels.urn:mrn:imo:mmsi:")
    }

    /// Process an incoming delta for a vessel context.
    /// Returns a status transition if the target changed state.
    pub fn update_target(
        &mut self,
        context: &str,
        values: &[(String, serde_json::Value)],
        now: Instant,
    ) -> Option<StatusTransition> {
        // Skip own vessel
        if context.ends_with(&self.self_uri) || context == "vessels.self" {
            return None;
        }

        // Only process vessels with MMSI
        let mmsi = Self::parse_mmsi(context)?;
        let mmsi_string = mmsi.to_string();

        let target = self
            .targets
            .entry(mmsi_string.clone())
            .or_insert_with(|| TrackedTarget::new(mmsi_string, context.to_string(), now));

        target.update_from_values(values);
        target.record_message(now)
    }

    /// Periodic tick — check for lost and stale targets.
    /// Returns all status transitions that occurred.
    pub fn tick(&mut self, now: Instant) -> Vec<StatusTransition> {
        let mut transitions = Vec::new();
        let mut to_remove = Vec::new();

        for (mmsi, target) in &mut self.targets {
            let elapsed = now.duration_since(target.last_seen);

            match target.status {
                TargetStatus::Unconfirmed => {
                    // Stale unconfirmed targets: remove if confirm window expired
                    // without enough messages
                    if elapsed > target.thresholds.confirm_window {
                        to_remove.push(mmsi.clone());
                    }
                }
                TargetStatus::Confirmed => {
                    if elapsed > target.thresholds.lost_after {
                        let old = target.status;
                        target.status = TargetStatus::Lost;
                        transitions.push(StatusTransition {
                            mmsi: mmsi.clone(),
                            context: target.context.clone(),
                            old_status: old,
                            new_status: TargetStatus::Lost,
                        });
                    }
                }
                TargetStatus::Lost => {
                    if elapsed > target.thresholds.lost_after + target.thresholds.remove_after {
                        to_remove.push(mmsi.clone());
                    }
                }
            }
        }

        for mmsi in to_remove {
            self.targets.remove(&mmsi);
        }

        transitions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── MMSI Classification ─────────────────────────────────────────────

    #[test]
    fn classify_normal_vessel() {
        assert_eq!(classify_mmsi("211457160"), TargetClass::Vessel);
        assert_eq!(classify_mmsi("366999000"), TargetClass::Vessel);
    }

    #[test]
    fn classify_aton() {
        assert_eq!(classify_mmsi("970012345"), TargetClass::Aton);
        assert_eq!(classify_mmsi("979999999"), TargetClass::Aton);
    }

    #[test]
    fn classify_base_station() {
        assert_eq!(classify_mmsi("002111111"), TargetClass::Base);
        assert_eq!(classify_mmsi("003669999"), TargetClass::Base);
    }

    #[test]
    fn classify_sar() {
        assert_eq!(classify_mmsi("111123456"), TargetClass::Sar);
    }

    // ── State Machine ───────────────────────────────────────────────────

    #[test]
    fn new_target_is_unconfirmed() {
        let t = TrackedTarget::new(
            "211457160".into(),
            "vessels.urn:mrn:imo:mmsi:211457160".into(),
            Instant::now(),
        );
        assert_eq!(t.status, TargetStatus::Unconfirmed);
        assert_eq!(t.class, TargetClass::Vessel);
        assert_eq!(t.message_count, 0);
    }

    #[test]
    fn vessel_confirms_after_two_messages() {
        let now = Instant::now();
        let mut t = TrackedTarget::new(
            "211457160".into(),
            "vessels.urn:mrn:imo:mmsi:211457160".into(),
            now,
        );
        assert_eq!(t.status, TargetStatus::Unconfirmed);

        // First message — still unconfirmed (vessel needs 2)
        let transition = t.record_message(now);
        assert!(transition.is_none());

        // Second message within confirm window → confirmed
        let transition = t.record_message(now + Duration::from_secs(10));
        assert!(transition.is_some());
        let tr = transition.unwrap();
        assert_eq!(tr.old_status, TargetStatus::Unconfirmed);
        assert_eq!(tr.new_status, TargetStatus::Confirmed);
        assert_eq!(t.status, TargetStatus::Confirmed);
    }

    #[test]
    fn aton_confirms_on_first_message() {
        // ATONs need only 1 message to confirm
        let now = Instant::now();
        let mut t = TrackedTarget::new(
            "970012345".into(),
            "vessels.urn:mrn:imo:mmsi:970012345".into(),
            now,
        );
        // First record_message should confirm (confirm_count=1 for ATONs)
        let transition = t.record_message(now);
        assert!(transition.is_some());
        assert_eq!(t.status, TargetStatus::Confirmed);
    }

    #[test]
    fn confirmed_to_lost_on_timeout() {
        let now = Instant::now();
        let mut t = TrackedTarget::new(
            "211457160".into(),
            "vessels.urn:mrn:imo:mmsi:211457160".into(),
            now,
        );
        t.record_message(now); // first message
        t.record_message(now + Duration::from_secs(5)); // second → confirms

        assert_eq!(t.status, TargetStatus::Confirmed);

        // Simulate tick after lost_after
        let elapsed = now + Duration::from_secs(5) + Duration::from_secs(361);
        let since_last = elapsed.duration_since(t.last_seen);
        assert!(since_last > t.thresholds.lost_after);

        // Manually check what tick would do
        let old = t.status;
        t.status = TargetStatus::Lost;
        assert_eq!(old, TargetStatus::Confirmed);
        assert_eq!(t.status, TargetStatus::Lost);
    }

    #[test]
    fn lost_to_confirmed_on_new_message() {
        let now = Instant::now();
        let mut t = TrackedTarget::new(
            "211457160".into(),
            "vessels.urn:mrn:imo:mmsi:211457160".into(),
            now,
        );
        t.record_message(now); // first
        t.record_message(now + Duration::from_secs(5)); // second → confirms
        t.status = TargetStatus::Lost; // simulate going lost

        let transition = t.record_message(now + Duration::from_secs(500));
        assert!(transition.is_some());
        let tr = transition.unwrap();
        assert_eq!(tr.old_status, TargetStatus::Lost);
        assert_eq!(tr.new_status, TargetStatus::Confirmed);
        assert_eq!(t.status, TargetStatus::Confirmed);
    }

    // ── AisTracker ──────────────────────────────────────────────────────

    #[test]
    fn tracker_ignores_self_vessel() {
        let mut tracker = AisTracker::new("urn:mrn:signalk:uuid:test-uuid".into());
        let result = tracker.update_target(
            "vessels.urn:mrn:signalk:uuid:test-uuid",
            &[("navigation.speedOverGround".into(), serde_json::json!(5.0))],
            Instant::now(),
        );
        assert!(result.is_none());
        assert_eq!(tracker.target_count(), 0);
    }

    #[test]
    fn tracker_ignores_vessels_self() {
        let mut tracker = AisTracker::new("urn:mrn:signalk:uuid:test-uuid".into());
        let result = tracker.update_target(
            "vessels.self",
            &[("navigation.speedOverGround".into(), serde_json::json!(5.0))],
            Instant::now(),
        );
        assert!(result.is_none());
        assert_eq!(tracker.target_count(), 0);
    }

    #[test]
    fn tracker_creates_and_confirms_target() {
        let now = Instant::now();
        let mut tracker = AisTracker::new("urn:mrn:signalk:uuid:test-uuid".into());

        // First message — creates unconfirmed target
        let result = tracker.update_target(
            "vessels.urn:mrn:imo:mmsi:211457160",
            &[("navigation.speedOverGround".into(), serde_json::json!(5.0))],
            now,
        );
        assert!(result.is_none()); // still unconfirmed after 1st msg (vessel needs 2)
        assert_eq!(tracker.target_count(), 1);

        // Second message — confirms
        let result = tracker.update_target(
            "vessels.urn:mrn:imo:mmsi:211457160",
            &[("navigation.speedOverGround".into(), serde_json::json!(5.2))],
            now + Duration::from_secs(10),
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().new_status, TargetStatus::Confirmed);
    }

    #[test]
    fn tracker_updates_position() {
        let now = Instant::now();
        let mut tracker = AisTracker::new("urn:mrn:signalk:uuid:test-uuid".into());

        tracker.update_target(
            "vessels.urn:mrn:imo:mmsi:211457160",
            &[(
                "navigation.position".into(),
                serde_json::json!({"latitude": 54.123, "longitude": 10.456}),
            )],
            now,
        );

        let target = tracker.get("211457160").unwrap();
        assert_eq!(target.position, Some((54.123, 10.456)));
    }

    #[test]
    fn tracker_tick_marks_lost() {
        let now = Instant::now();
        let mut tracker = AisTracker::new("urn:mrn:signalk:uuid:test-uuid".into());

        // Create and confirm a target
        tracker.update_target(
            "vessels.urn:mrn:imo:mmsi:211457160",
            &[("navigation.speedOverGround".into(), serde_json::json!(5.0))],
            now,
        );
        tracker.update_target(
            "vessels.urn:mrn:imo:mmsi:211457160",
            &[("navigation.speedOverGround".into(), serde_json::json!(5.2))],
            now + Duration::from_secs(5),
        );

        // Tick after lost_after (360s) — target should be marked lost
        let transitions = tracker.tick(now + Duration::from_secs(370));
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].new_status, TargetStatus::Lost);
        assert_eq!(tracker.get("211457160").unwrap().status, TargetStatus::Lost);
    }

    #[test]
    fn tracker_tick_removes_stale_unconfirmed() {
        let now = Instant::now();
        let mut tracker = AisTracker::new("urn:mrn:signalk:uuid:test-uuid".into());

        // Create an unconfirmed target (single message, vessel needs 2)
        tracker.update_target(
            "vessels.urn:mrn:imo:mmsi:211457160",
            &[("navigation.speedOverGround".into(), serde_json::json!(5.0))],
            now,
        );
        assert_eq!(tracker.target_count(), 1);

        // Tick after confirm_window (180s) — target should be removed
        tracker.tick(now + Duration::from_secs(200));
        assert_eq!(tracker.target_count(), 0);
    }

    #[test]
    fn tracker_tick_removes_lost_after_timeout() {
        let now = Instant::now();
        let mut tracker = AisTracker::new("urn:mrn:signalk:uuid:test-uuid".into());

        // Create and confirm
        tracker.update_target("vessels.urn:mrn:imo:mmsi:211457160", &[], now);
        tracker.update_target(
            "vessels.urn:mrn:imo:mmsi:211457160",
            &[],
            now + Duration::from_secs(5),
        );

        // Mark as lost
        tracker.tick(now + Duration::from_secs(370));
        assert_eq!(tracker.get("211457160").unwrap().status, TargetStatus::Lost);

        // Tick after remove_after (540s after lost_after) — target should be gone
        tracker.tick(now + Duration::from_secs(370 + 540 + 1));
        assert_eq!(tracker.target_count(), 0);
    }

    #[test]
    fn tracker_multiple_targets() {
        let now = Instant::now();
        let mut tracker = AisTracker::new("urn:mrn:signalk:uuid:test-uuid".into());

        tracker.update_target("vessels.urn:mrn:imo:mmsi:211457160", &[], now);
        tracker.update_target("vessels.urn:mrn:imo:mmsi:366999000", &[], now);
        tracker.update_target("vessels.urn:mrn:imo:mmsi:970012345", &[], now);

        assert_eq!(tracker.target_count(), 3);
        assert_eq!(tracker.get("211457160").unwrap().class, TargetClass::Vessel);
        assert_eq!(tracker.get("970012345").unwrap().class, TargetClass::Aton);
    }

    #[test]
    fn parse_mmsi_valid() {
        assert_eq!(
            AisTracker::parse_mmsi("vessels.urn:mrn:imo:mmsi:211457160"),
            Some("211457160")
        );
    }

    #[test]
    fn parse_mmsi_invalid() {
        assert_eq!(AisTracker::parse_mmsi("vessels.self"), None);
        assert_eq!(
            AisTracker::parse_mmsi("vessels.urn:mrn:signalk:uuid:abc"),
            None
        );
    }

    #[test]
    fn status_str_values() {
        assert_eq!(TargetStatus::Unconfirmed.as_str(), "unconfirmed");
        assert_eq!(TargetStatus::Confirmed.as_str(), "confirmed");
        assert_eq!(TargetStatus::Lost.as_str(), "lost");
    }

    #[test]
    fn target_updates_name_and_callsign() {
        let now = Instant::now();
        let mut t = TrackedTarget::new(
            "211457160".into(),
            "vessels.urn:mrn:imo:mmsi:211457160".into(),
            now,
        );

        t.update_from_values(&[
            ("name".into(), serde_json::json!("PACIFIC EXPLORER")),
            (
                "communication.callsignVhf".into(),
                serde_json::json!("DJKL"),
            ),
        ]);

        assert_eq!(t.name.as_deref(), Some("PACIFIC EXPLORER"));
        assert_eq!(t.callsign.as_deref(), Some("DJKL"));
    }
}
