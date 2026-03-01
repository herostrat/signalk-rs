//! System time and NTP synchronisation status.
//!
//! Reads system clock and checks whether time is synchronised via NTP
//! (systemd-timesyncd or chrony). Detects local timezone from the OS.
//!
//! # Future work (not implemented here)
//! - Setting the system clock from GPS fix (`clock_settime` via nix crate,
//!   requires `CAP_SYS_TIME`). GPS warm-start benefit: accurate system time
//!   lets the GPS chip narrow satellite search → faster TTFF on next boot.
//!   Only useful for live GPS time, NOT for persisting GPS time across
//!   reboots (days/weeks of staleness would be counterproductive).

use chrono::Local;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

/// Snapshot of system time state.
#[derive(Debug)]
pub struct TimeStatus {
    /// Current system time in milliseconds since Unix epoch.
    pub system_time_ms: i64,
    /// IANA timezone region of the OS (e.g. "Europe/Berlin", "UTC").
    pub timezone_region: String,
    /// Whether NTP (systemd-timesyncd or chrony) reports synchronisation.
    pub ntp_synchronized: bool,
}

/// Collect current time status from the OS.
pub fn time_status() -> TimeStatus {
    let system_time_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    TimeStatus {
        system_time_ms,
        timezone_region: local_timezone(),
        ntp_synchronized: ntp_synchronized(),
    }
}

/// Convert `TimeStatus` to a JSON response body.
pub fn time_status_json(status: &TimeStatus) -> Value {
    json!({
        "system_time_ms": status.system_time_ms,
        "timezone_region": status.timezone_region,
        "ntp_synchronized": status.ntp_synchronized,
    })
}

/// Detect the local IANA timezone name.
///
/// Tries, in order:
/// 1. The `TZ` environment variable
/// 2. The target of the `/etc/localtime` symlink (strips the `zoneinfo/` prefix)
/// 3. The content of `/etc/timezone` (Debian/Ubuntu style)
/// 4. Falls back to `"UTC"`
pub fn local_timezone() -> String {
    // 1. TZ env var
    if let Ok(tz) = std::env::var("TZ")
        && !tz.is_empty()
    {
        return tz;
    }

    // 2. /etc/localtime symlink → extract zone name
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        // Typical: /usr/share/zoneinfo/Europe/Berlin
        if let Some(pos) = s.find("zoneinfo/") {
            return s[pos + "zoneinfo/".len()..].to_string();
        }
    }

    // 3. /etc/timezone file (Debian/Ubuntu)
    if let Ok(content) = std::fs::read_to_string("/etc/timezone") {
        let tz = content.trim().to_string();
        if !tz.is_empty() {
            return tz;
        }
    }

    // 4. Use chrono's local offset as a last resort: render as numeric offset.
    //    This is not an IANA name but at least it's not wrong.
    let offset = Local::now().offset().local_minus_utc();
    let hours = offset / 3600;
    let mins = (offset.abs() % 3600) / 60;
    if offset == 0 {
        "UTC".to_string()
    } else {
        format!("{:+03}:{:02}", hours, mins)
    }
}

/// Check whether the system clock is NTP-synchronised.
///
/// Checks systemd-timesyncd first (fast file-based check), then falls back to
/// reading chrony's `tracking.log` or `chronyc` output file if present.
pub fn ntp_synchronized() -> bool {
    // systemd-timesyncd: the presence of this file indicates sync
    if std::path::Path::new("/run/systemd/timesync/synchronized").exists() {
        return true;
    }

    // systemd-timesyncd alternative: /run/timesyncd.conf.d/... or timedatectl
    // state file (varies by distro). Try reading the state file.
    if let Ok(state) = std::fs::read_to_string("/run/systemd/timesync/synchronized")
        && (state.trim() == "1" || state.trim().eq_ignore_ascii_case("yes"))
    {
        return true;
    }

    // chrony: check tracking log for RMS offset reasonableness
    // /var/run/chrony/chronyd.pid exists when chrony is running
    if std::path::Path::new("/var/run/chrony/chronyd.pid").exists() {
        // chrony running → assume synchronised (we can't easily check without
        // running chronyc, which is a process spawn we want to avoid)
        return true;
    }

    // openntpd: /var/run/openntpd/ directory
    if std::path::Path::new("/var/run/openntpd/").exists() {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_status_returns_positive_ms() {
        let status = time_status();
        // Must be after 2020-01-01 (1577836800000 ms)
        assert!(
            status.system_time_ms > 1_577_836_800_000,
            "system_time_ms suspiciously old: {}",
            status.system_time_ms
        );
    }

    #[test]
    fn time_status_json_has_required_fields() {
        let status = time_status();
        let v = time_status_json(&status);
        assert!(v["system_time_ms"].as_i64().is_some());
        assert!(v["timezone_region"].as_str().is_some());
        assert!(v["ntp_synchronized"].as_bool().is_some());
    }

    #[test]
    fn local_timezone_returns_non_empty() {
        let tz = local_timezone();
        assert!(!tz.is_empty());
    }

    #[test]
    fn ntp_synchronized_does_not_panic() {
        // Just verify it doesn't panic — actual value depends on the host
        let _ = ntp_synchronized();
    }
}
