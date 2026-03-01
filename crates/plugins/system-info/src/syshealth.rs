//! System health metrics: CPU load, RAM, disk space, uptime, CPU temperature.
//!
//! All data is read from Linux `/proc` and `/sys` virtual filesystems on-demand.
//! No background tasks needed — these reads are cheap kernel operations.

use serde_json::{Value, json};
use std::fs;

/// Collected system health snapshot.
#[derive(Debug)]
pub struct SysHealth {
    pub loadavg_1m: f64,
    pub loadavg_5m: f64,
    pub loadavg_15m: f64,
    pub memory_total_bytes: u64,
    pub memory_free_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_free_bytes: u64,
    pub uptime_s: f64,
    /// CPU temperature in degrees Celsius, if available.
    pub cpu_temp_celsius: Option<f64>,
}

/// Collect system health metrics.
pub fn sys_health() -> SysHealth {
    let (load1, load5, load15) = read_loadavg();
    let (mem_total, mem_free) = read_meminfo();
    let (disk_total, disk_free) = read_disk_usage("/");
    let uptime = read_uptime();
    let cpu_temp = read_cpu_temp();

    SysHealth {
        loadavg_1m: load1,
        loadavg_5m: load5,
        loadavg_15m: load15,
        memory_total_bytes: mem_total,
        memory_free_bytes: mem_free,
        disk_total_bytes: disk_total,
        disk_free_bytes: disk_free,
        uptime_s: uptime,
        cpu_temp_celsius: cpu_temp,
    }
}

/// Convert `SysHealth` to a JSON response body.
pub fn sys_health_json(h: &SysHealth) -> Value {
    let mut v = json!({
        "loadavg": {
            "1m":  h.loadavg_1m,
            "5m":  h.loadavg_5m,
            "15m": h.loadavg_15m,
        },
        "memory": {
            "total_bytes": h.memory_total_bytes,
            "free_bytes":  h.memory_free_bytes,
        },
        "disk": {
            "total_bytes": h.disk_total_bytes,
            "free_bytes":  h.disk_free_bytes,
        },
        "uptime_s": h.uptime_s,
    });
    if let Some(temp) = h.cpu_temp_celsius {
        v["cpu_temp_celsius"] = json!(temp);
    }
    v
}

/// Read load averages from `/proc/loadavg`.
///
/// Format: `0.12 0.08 0.05 1/423 12345`
fn read_loadavg() -> (f64, f64, f64) {
    let content = match fs::read_to_string("/proc/loadavg") {
        Ok(c) => c,
        Err(_) => return (0.0, 0.0, 0.0),
    };
    let parts: Vec<&str> = content.split_whitespace().collect();
    let parse = |s: &str| s.parse::<f64>().unwrap_or(0.0);
    (
        parts.first().map(|s| parse(s)).unwrap_or(0.0),
        parts.get(1).map(|s| parse(s)).unwrap_or(0.0),
        parts.get(2).map(|s| parse(s)).unwrap_or(0.0),
    )
}

/// Read memory info from `/proc/meminfo`.
///
/// Returns (total_bytes, free_bytes). Uses `MemAvailable` for free if present
/// (more accurate for "usable free" than `MemFree`).
fn read_meminfo() -> (u64, u64) {
    let content = match fs::read_to_string("/proc/meminfo") {
        Ok(c) => c,
        Err(_) => return (0, 0),
    };

    let mut total_kb: u64 = 0;
    let mut free_kb: u64 = 0;
    let mut available_kb: Option<u64> = None;

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemFree:") {
            free_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available_kb = Some(parse_kb(rest));
        }
    }

    (total_kb * 1024, available_kb.unwrap_or(free_kb) * 1024)
}

/// Parse a `/proc/meminfo` value line like `   4096 kB` into kilobytes.
fn parse_kb(s: &str) -> u64 {
    s.split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Read disk usage for a mount point using `statvfs`.
///
/// Returns (total_bytes, free_bytes). Uses `f_bavail` (available to
/// non-root processes) rather than `f_bfree` for the free count.
fn read_disk_usage(path: &str) -> (u64, u64) {
    // Use libc statvfs via the nix-free approach: read /proc/mounts and
    // /proc/diskstats is not the right approach. Use std::fs::metadata?
    // Actually, statvfs is the correct syscall. We call it via libc directly.
    //
    // To avoid the nix dependency in this phase, we read from /proc/mounts
    // and fall back to 0 if we can't compute. However, statvfs is straightforward
    // via a tiny unsafe block using libc.
    unsafe {
        let mut buf: libc::statvfs = std::mem::zeroed();
        let path_c = std::ffi::CString::new(path).unwrap_or_default();
        if libc::statvfs(path_c.as_ptr(), &mut buf) == 0 {
            let total = buf.f_blocks * buf.f_frsize;
            let free = buf.f_bavail * buf.f_frsize;
            (total, free)
        } else {
            (0, 0)
        }
    }
}

/// Read system uptime in seconds from `/proc/uptime`.
///
/// Format: `86400.12 43200.06` (uptime, idle time)
fn read_uptime() -> f64 {
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|n| n.parse().ok()))
        .unwrap_or(0.0)
}

/// Read CPU temperature from `/sys/class/thermal/thermal_zone*/temp`.
///
/// Returns the first valid reading converted to °C (the raw value is in m°C).
/// Tries thermal zones in order 0–9, prefers zones labelled "cpu-thermal" or "soc".
fn read_cpu_temp() -> Option<f64> {
    // First try to find a zone with a CPU-related type label
    for zone in 0..10u32 {
        let base = format!("/sys/class/thermal/thermal_zone{zone}");
        let type_path = format!("{base}/type");
        if let Ok(zone_type) = fs::read_to_string(&type_path) {
            let t = zone_type.trim().to_lowercase();
            if (t.contains("cpu") || t.contains("soc") || t.contains("x86_pkg"))
                && let Some(temp) = read_thermal_temp(&base)
            {
                return Some(temp);
            }
        }
    }

    // Fall back to thermal_zone0
    read_thermal_temp("/sys/class/thermal/thermal_zone0")
}

fn read_thermal_temp(base: &str) -> Option<f64> {
    let temp_raw: i64 = fs::read_to_string(format!("{base}/temp"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    // Raw value is in millidegrees Celsius
    Some(temp_raw as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sys_health_does_not_panic() {
        let h = sys_health();
        // uptime should be positive on any running system
        assert!(h.uptime_s >= 0.0);
    }

    #[test]
    fn sys_health_json_has_required_fields() {
        let h = sys_health();
        let v = sys_health_json(&h);
        assert!(v["loadavg"]["1m"].as_f64().is_some());
        assert!(v["loadavg"]["5m"].as_f64().is_some());
        assert!(v["loadavg"]["15m"].as_f64().is_some());
        assert!(v["memory"]["total_bytes"].as_u64().is_some());
        assert!(v["memory"]["free_bytes"].as_u64().is_some());
        assert!(v["disk"]["total_bytes"].as_u64().is_some());
        assert!(v["disk"]["free_bytes"].as_u64().is_some());
        assert!(v["uptime_s"].as_f64().is_some());
    }

    #[test]
    fn memory_total_is_nonzero() {
        let (total, _free) = read_meminfo();
        assert!(total > 0, "MemTotal should be > 0 on any real system");
    }

    #[test]
    fn disk_total_is_nonzero_for_root() {
        let (total, _free) = read_disk_usage("/");
        assert!(total > 0, "disk total for / should be > 0");
    }

    #[test]
    fn uptime_is_positive() {
        let uptime = read_uptime();
        assert!(uptime > 0.0, "uptime should be positive: {uptime}");
    }

    #[test]
    fn loadavg_parses() {
        let (l1, l5, l15) = read_loadavg();
        assert!(l1 >= 0.0);
        assert!(l5 >= 0.0);
        assert!(l15 >= 0.0);
    }

    #[test]
    fn cpu_temp_is_reasonable_if_present() {
        if let Some(temp) = read_cpu_temp() {
            // Reasonable range: -20°C to 120°C
            assert!(
                (-20.0..=120.0).contains(&temp),
                "CPU temp out of range: {temp}°C"
            );
        }
        // If None, that's OK — not all systems expose thermal zones
    }
}
