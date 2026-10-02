use std::{fs, path::Path, time::Duration};

use chrono::{Local, Timelike};
use serde_json::{Value, json};

pub const CLOCK_SOURCE: &str = "clock";
pub const CPU_SOURCE: &str = "cpu";
pub const MEMORY_SOURCE: &str = "memory";
pub const BATTERY_SOURCE: &str = "battery";
pub const BACKLIGHT_SOURCE: &str = "backlight";

pub const CLOCK_INTERVAL: Duration = Duration::from_secs(60);
pub const MEMORY_INTERVAL: Duration = Duration::from_secs(10);
pub const CPU_INTERVAL: Duration = Duration::from_secs(3);
pub const BATTERY_INTERVAL: Duration = Duration::from_secs(30);
pub const BACKLIGHT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuSnapshot {
    pub total: u64,
    pub idle: u64,
}

impl CpuSnapshot {
    pub fn usage_percent(self, previous: Self) -> u64 {
        let total_delta = self.total.saturating_sub(previous.total);
        let idle_delta = self.idle.saturating_sub(previous.idle);
        let active_delta = total_delta.saturating_sub(idle_delta);
        if total_delta == 0 {
            0
        } else {
            active_delta
                .saturating_mul(100)
                .checked_div(total_delta)
                .unwrap_or_default()
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub total_kib: u64,
    pub available_kib: u64,
}

impl MemorySnapshot {
    pub fn used_kib(&self) -> u64 {
        self.total_kib.saturating_sub(self.available_kib)
    }

    pub fn percent(&self) -> u64 {
        if self.total_kib == 0 {
            0
        } else {
            self.used_kib()
                .saturating_mul(100)
                .checked_div(self.total_kib)
                .unwrap_or_default()
        }
    }
}

pub fn is_builtin_source(id: &str) -> bool {
    matches!(
        id,
        CLOCK_SOURCE | CPU_SOURCE | MEMORY_SOURCE | BATTERY_SOURCE | BACKLIGHT_SOURCE
    )
}

pub fn default_interval(id: &str) -> Option<Duration> {
    match id {
        CLOCK_SOURCE => Some(CLOCK_INTERVAL),
        CPU_SOURCE => Some(CPU_INTERVAL),
        MEMORY_SOURCE => Some(MEMORY_INTERVAL),
        BATTERY_SOURCE => Some(BATTERY_INTERVAL),
        BACKLIGHT_SOURCE => Some(BACKLIGHT_INTERVAL),
        _ => None,
    }
}

pub fn supports_event(id: &str, event: &str) -> bool {
    match id {
        CLOCK_SOURCE => event == "tick",
        CPU_SOURCE | MEMORY_SOURCE | BATTERY_SOURCE | BACKLIGHT_SOURCE => event == "sample",
        _ => false,
    }
}

pub fn next_delay(id: &str, interval: Duration) -> Duration {
    if id == CLOCK_SOURCE {
        let now = Local::now();
        let interval_millis = interval.as_millis();
        if interval_millis >= 1000 && interval_millis.is_multiple_of(1000) {
            let interval_secs = (interval_millis / 1000) as u64;
            let current_sec = now.second() as u64;
            let current_nanos = now.nanosecond() as u64;
            let rem_secs = interval_secs - 1 - (current_sec % interval_secs);
            let rem_nanos = 1_000_000_000u64.saturating_sub(current_nanos);
            return Duration::from_secs(rem_secs)
                + Duration::from_nanos(rem_nanos)
                + Duration::from_millis(5);
        }
    }
    interval
}

pub fn sample(
    id: &str,
    previous_cpu: &mut Option<CpuSnapshot>,
) -> Result<(&'static str, Value), String> {
    match id {
        CLOCK_SOURCE => Ok(("tick", json!({"timestamp": Local::now().timestamp()}))),
        CPU_SOURCE => {
            let snapshot = read_cpu_snapshot()?;
            let percent = previous_cpu
                .map(|previous| snapshot.usage_percent(previous))
                .unwrap_or_default();
            *previous_cpu = Some(snapshot);
            Ok((
                "sample",
                json!({"percent": i64::try_from(percent).unwrap_or(i64::MAX)}),
            ))
        }
        MEMORY_SOURCE => {
            let contents =
                fs::read_to_string("/proc/meminfo").map_err(|error| error.to_string())?;
            let snapshot = parse_meminfo(&contents)?;
            Ok((
                "sample",
                json!({
                    "used_kib": i64::try_from(snapshot.used_kib()).unwrap_or(i64::MAX),
                    "available_kib": i64::try_from(snapshot.available_kib).unwrap_or(i64::MAX),
                    "total_kib": i64::try_from(snapshot.total_kib).unwrap_or(i64::MAX),
                    "percent": i64::try_from(snapshot.percent()).unwrap_or(i64::MAX),
                }),
            ))
        }
        BATTERY_SOURCE => {
            let snapshot = read_battery_snapshot();
            Ok((
                "sample",
                json!({
                    "present": snapshot.present,
                    "percent": i64::try_from(snapshot.percent).unwrap_or(i64::MAX),
                    "status": snapshot.status,
                    "plugged": snapshot.plugged,
                    "icon": snapshot.icon,
                }),
            ))
        }
        BACKLIGHT_SOURCE => {
            let snapshot = read_backlight_snapshot();
            Ok((
                "sample",
                json!({
                    "present": snapshot.present,
                    "percent": i64::try_from(snapshot.percent).unwrap_or(i64::MAX),
                    "brightness": i64::try_from(snapshot.brightness).unwrap_or(i64::MAX),
                    "max_brightness": i64::try_from(snapshot.max_brightness).unwrap_or(i64::MAX),
                    "device": snapshot.device,
                    "icon": snapshot.icon,
                }),
            ))
        }
        _ => Err(format!("unknown builtin source {id:?}")),
    }
}

pub fn read_cpu_snapshot() -> Result<CpuSnapshot, String> {
    let contents = fs::read_to_string("/proc/stat").map_err(|error| error.to_string())?;
    parse_cpu_snapshot(&contents)
}

pub fn parse_cpu_snapshot(contents: &str) -> Result<CpuSnapshot, String> {
    let line = contents
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| "aggregate cpu line is missing".to_owned())?;
    let values: Vec<_> = line[4..]
        .split_whitespace()
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .map_err(|error| format!("invalid aggregate cpu value: {error}"))?;
    if values.len() < 5 {
        return Err("aggregate cpu line has too few fields".to_owned());
    }
    let total = values.iter().copied().sum();
    let idle = values[3].saturating_add(values[4]);
    Ok(CpuSnapshot { total, idle })
}

pub fn parse_meminfo(contents: &str) -> Result<MemorySnapshot, String> {
    let mut total_kib = None;
    let mut available_kib = None;
    let mut free_kib = None;
    let mut buffers_kib = None;
    let mut cached_kib = None;

    for line in contents.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(value) = rest.split_whitespace().next() else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        match name {
            "MemTotal" => total_kib = Some(value),
            "MemAvailable" => available_kib = Some(value),
            "MemFree" => free_kib = Some(value),
            "Buffers" => buffers_kib = Some(value),
            "Cached" => cached_kib = Some(value),
            _ => {}
        }
    }

    let total_kib = total_kib.ok_or_else(|| "MemTotal is missing".to_owned())?;
    let available_kib = available_kib.unwrap_or_else(|| {
        free_kib
            .unwrap_or_default()
            .saturating_add(buffers_kib.unwrap_or_default())
            .saturating_add(cached_kib.unwrap_or_default())
    });
    Ok(MemorySnapshot {
        total_kib,
        available_kib: available_kib.min(total_kib),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatterySnapshot {
    pub present: bool,
    pub percent: u64,
    pub status: String,
    pub plugged: bool,
    pub icon: &'static str,
}

pub fn battery_icon(status: &str, percent: u64, plugged: bool) -> &'static str {
    if status.eq_ignore_ascii_case("charging") {
        "󰂄"
    } else if status.eq_ignore_ascii_case("full") {
        "󰁹"
    } else if plugged {
        "󰂄"
    } else {
        match percent {
            90..=100 => "󰁹",
            70..=89 => "󰂀",
            50..=69 => "󰁾",
            30..=49 => "󰁼",
            15..=29 => "󰁺",
            _ => "󰂎",
        }
    }
}

pub fn read_battery_snapshot() -> BatterySnapshot {
    read_battery_snapshot_from(Path::new("/sys/class/power_supply"))
}

pub fn read_battery_snapshot_from(dir: &Path) -> BatterySnapshot {
    let Ok(entries) = fs::read_dir(dir) else {
        return BatterySnapshot {
            present: false,
            percent: 100,
            status: "AC".to_owned(),
            plugged: true,
            icon: "󰚥",
        };
    };

    let mut batteries = Vec::new();
    let mut plugged = false;

    for entry in entries.flatten() {
        let path = entry.path();
        let supply_type = fs::read_to_string(path.join("type"))
            .map(|s| s.trim().to_lowercase())
            .unwrap_or_default();

        if supply_type == "mains" || supply_type.starts_with("usb") {
            if let Ok(online) = fs::read_to_string(path.join("online"))
                && online.trim() == "1"
            {
                plugged = true;
            }
        } else if supply_type == "battery" {
            let capacity = fs::read_to_string(path.join("capacity"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok());
            let status = fs::read_to_string(path.join("status"))
                .map(|s| s.trim().to_owned())
                .unwrap_or_else(|_| "Discharging".to_owned());
            if let Some(cap) = capacity {
                batteries.push((cap.min(100), status));
            }
        }
    }

    if batteries.is_empty() {
        return BatterySnapshot {
            present: false,
            percent: 100,
            status: "AC".to_owned(),
            plugged: true,
            icon: "󰚥",
        };
    }

    let avg_percent = batteries.iter().map(|(cap, _)| *cap).sum::<u64>() / batteries.len() as u64;
    let any_charging = batteries
        .iter()
        .any(|(_, st)| st.eq_ignore_ascii_case("charging"));
    let status = if any_charging {
        "Charging".to_owned()
    } else {
        batteries[0].1.clone()
    };
    let is_plugged = plugged || any_charging || status.eq_ignore_ascii_case("full");
    let icon = battery_icon(&status, avg_percent, is_plugged);

    BatterySnapshot {
        present: true,
        percent: avg_percent,
        status,
        plugged: is_plugged,
        icon,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklightSnapshot {
    pub present: bool,
    pub percent: u64,
    pub brightness: u64,
    pub max_brightness: u64,
    pub device: String,
    pub icon: &'static str,
}

pub fn backlight_icon(percent: u64) -> &'static str {
    match percent {
        70..=100 => "󰃠",
        35..=69 => "󰃟",
        _ => "󰃞",
    }
}

pub fn read_backlight_snapshot() -> BacklightSnapshot {
    read_backlight_snapshot_from(Path::new("/sys/class/backlight"))
}

pub fn read_backlight_snapshot_from(dir: &Path) -> BacklightSnapshot {
    let Ok(entries) = fs::read_dir(dir) else {
        return BacklightSnapshot {
            present: false,
            percent: 100,
            brightness: 100,
            max_brightness: 100,
            device: "none".to_owned(),
            icon: "󰃠",
        };
    };

    let mut found = None;
    let mut dir_entries: Vec<_> = entries.flatten().collect();
    dir_entries.sort_by_key(|e| e.file_name());

    for entry in dir_entries {
        let path = entry.path();
        let brightness = fs::read_to_string(path.join("brightness"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        let max_brightness = fs::read_to_string(path.join("max_brightness"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());

        if let (Some(b), Some(m)) = (brightness, max_brightness)
            && m > 0
        {
            let percent = (b.saturating_mul(100).saturating_add(m / 2) / m).min(100);
            let dev_name = entry.file_name().to_string_lossy().into_owned();
            let icon = backlight_icon(percent);
            found = Some(BacklightSnapshot {
                present: true,
                percent,
                brightness: b,
                max_brightness: m,
                device: dev_name,
                icon,
            });
            break;
        }
    }

    found.unwrap_or(BacklightSnapshot {
        present: false,
        percent: 100,
        brightness: 100,
        max_brightness: 100,
        device: "none".to_owned(),
        icon: "󰃠",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_calculates_cpu_usage() {
        let previous = parse_cpu_snapshot("cpu  100 0 100 700 100 0 0 0\n").expect("previous cpu");
        let current = parse_cpu_snapshot("cpu  130 0 130 790 100 0 0 0\n").expect("current cpu");

        assert_eq!(current.usage_percent(previous), 40);
    }

    #[test]
    fn parses_memory_info() {
        let snapshot = parse_meminfo("MemTotal:       16384000 kB\nMemAvailable:    8192000 kB\n")
            .expect("valid meminfo");

        assert_eq!(
            snapshot,
            MemorySnapshot {
                total_kib: 16_384_000,
                available_kib: 8_192_000,
            }
        );
        assert_eq!(snapshot.used_kib(), 8_192_000);
        assert_eq!(snapshot.percent(), 50);
    }

    #[test]
    fn falls_back_to_basic_memory_fields() {
        let snapshot =
            parse_meminfo("MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 200 kB\nCached: 300 kB\n")
                .expect("valid fallback meminfo");

        assert_eq!(snapshot.available_kib, 600);
    }

    #[test]
    fn next_delay_aligns_clock_to_boundary() {
        let delay = next_delay(CLOCK_SOURCE, Duration::from_secs(60));
        assert!(delay <= Duration::from_secs(60) + Duration::from_millis(50));
        assert!(delay >= Duration::from_millis(5));

        let cpu_delay = next_delay(CPU_SOURCE, Duration::from_secs(3));
        assert_eq!(cpu_delay, Duration::from_secs(3));
    }

    #[test]
    fn reads_battery_and_backlight_snapshots() {
        let temp = std::env::temp_dir().join(format!("cellbar-sysfs-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let power_dir = temp.join("power_supply");
        let bat0 = power_dir.join("BAT0");
        let ac = power_dir.join("AC");
        fs::create_dir_all(&bat0).unwrap();
        fs::create_dir_all(&ac).unwrap();
        fs::write(bat0.join("type"), "Battery\n").unwrap();
        fs::write(bat0.join("capacity"), "78\n").unwrap();
        fs::write(bat0.join("status"), "Discharging\n").unwrap();
        fs::write(ac.join("type"), "Mains\n").unwrap();
        fs::write(ac.join("online"), "0\n").unwrap();

        let bat_snap = read_battery_snapshot_from(&power_dir);
        assert!(bat_snap.present);
        assert_eq!(bat_snap.percent, 78);
        assert_eq!(bat_snap.status, "Discharging");
        assert!(!bat_snap.plugged);
        assert_eq!(bat_snap.icon, "󰂀");

        let bl_dir = temp.join("backlight");
        let intel = bl_dir.join("intel_backlight");
        fs::create_dir_all(&intel).unwrap();
        fs::write(intel.join("brightness"), "450\n").unwrap();
        fs::write(intel.join("max_brightness"), "1000\n").unwrap();

        let bl_snap = read_backlight_snapshot_from(&bl_dir);
        assert!(bl_snap.present);
        assert_eq!(bl_snap.percent, 45);
        assert_eq!(bl_snap.brightness, 450);
        assert_eq!(bl_snap.max_brightness, 1000);
        assert_eq!(bl_snap.device, "intel_backlight");
        assert_eq!(bl_snap.icon, "󰃟");

        let _ = fs::remove_dir_all(&temp);
    }
}
