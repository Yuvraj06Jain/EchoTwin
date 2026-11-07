//! This computer's battery, read from `/sys/class/power_supply` (no extra dependencies, works
//! on every distro). Sent to the phone so its home screen can show it.

use std::path::Path;

pub const DEFAULT_ROOT: &str = "/sys/class/power_supply";

/// (level 0–100, charging) of the laptop's own battery; `None` on a computer without one.
/// Batteries of mice, keyboards and headsets (`scope = Device`) are ignored. With several
/// laptop batteries the level is the average weighted by capacity when known.
pub fn read(root: &Path) -> Option<(u8, bool)> {
    let mut entries: Vec<_> = std::fs::read_dir(root).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let (mut weighted, mut total, mut charging, mut found) = (0f64, 0f64, false, false);
    for e in entries {
        let p = e.path();
        let get = |f: &str| std::fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_string());
        if get("type").as_deref() != Some("Battery") || get("scope").as_deref() == Some("Device") {
            continue;
        }
        let Some(level) = get("capacity").and_then(|c| c.parse::<f64>().ok()) else {
            continue;
        };
        let weight = get("energy_full")
            .or_else(|| get("charge_full"))
            .and_then(|c| c.parse::<f64>().ok())
            .filter(|w| *w > 0.0)
            .unwrap_or(1.0);
        weighted += level.clamp(0.0, 100.0) * weight;
        total += weight;
        found = true;
        charging |= matches!(get("status").as_deref(), Some("Charging" | "Full"));
    }
    found.then(|| ((weighted / total).round() as u8, charging))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        for (f, v) in files {
            std::fs::write(d.join(f), format!("{v}\n")).unwrap();
        }
    }

    #[test]
    fn laptop_battery_only() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        assert_eq!(read(r), None, "desktop: no battery");
        supply(r, "AC", &[("type", "Mains"), ("online", "1")]);
        supply(r, "hidpp_battery_0", &[("type", "Battery"), ("scope", "Device"), ("capacity", "5")]);
        assert_eq!(read(r), None, "a mouse battery is not the laptop's");
        supply(r, "BAT0", &[("type", "Battery"), ("capacity", "76"), ("status", "Charging")]);
        assert_eq!(read(r), Some((76, true)));
        supply(r, "BAT0", &[("status", "Discharging")]);
        assert_eq!(read(r), Some((76, false)));
        // Two batteries: weighted by their size.
        supply(r, "BAT0", &[("energy_full", "30000000")]);
        supply(
            r,
            "BAT1",
            &[("type", "Battery"), ("capacity", "16"), ("energy_full", "10000000"), ("status", "Discharging")],
        );
        assert_eq!(read(r), Some((61, false)));
        assert_eq!(read(&r.join("missing")), None);
    }
}
