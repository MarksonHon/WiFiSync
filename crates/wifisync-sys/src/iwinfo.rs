//! iwinfo output collection (**read-only**): for LuCI display, never part of write decisions.

use crate::error::SysResult;
use crate::exec;
use crate::paths::Paths;
use serde_json::{json, Value};

/// Local wireless overview (use iwinfo if present, otherwise fall back to the sysfs count).
pub fn radio_status(paths: &Paths) -> SysResult<Value> {
    let phy_count = crate::sysfs::list_phys(paths).len();
    if !exec::has("iwinfo") {
        return Ok(json!({
            "available": false,
            "phy_count": phy_count,
            "devices": [],
            "note": "iwinfo is not installed, only the phy count is available",
        }));
    }

    let mut devices = Vec::new();
    if let Ok(out) = exec::run("iwinfo", &[]) {
        for line in out.stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // e.g. `wlan0     ESSID: "Home"` / `phy0 ...`
            let name = line
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            let mut detail = json!({ "name": name });
            if let Ok(info) = exec::run("iwinfo", &[&name, "info"]) {
                detail["info"] = json!(info.stdout);
            }
            devices.push(detail);
        }
    }

    Ok(json!({
        "available": true,
        "phy_count": phy_count,
        "devices": devices,
    }))
}

/// Capture a read-only state snapshot (written to the backup dir for later diagnosis).
pub fn capture_state(paths: &Paths) -> String {
    let mut out = String::new();
    let mut section = |title: &str, content: String| {
        out.push_str(&format!("\n===== {} =====\n{}", title, content));
    };

    if let Ok(r) = exec::run("ip", &["-o", "link"]) {
        section("ip -o link", r.stdout);
    }
    if let Ok(r) = exec::run("ip", &["-o", "addr"]) {
        section("ip -o addr", r.stdout);
    }
    if let Ok(r) = exec::run("ip", &["-o", "route"]) {
        section("ip -o route", r.stdout);
    }
    if let Ok(r) = radio_status(paths) {
        section(
            "iwinfo",
            serde_json::to_string_pretty(&r).unwrap_or_default(),
        );
    }
    let mut bridges = Vec::new();
    if let Ok(entries) = std::fs::read_dir(paths.root().join("sys/class/net")) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with("br-") || entry.path().join("bridge").exists() {
                    let members = crate::sysfs::bridge_members(paths, name);
                    bridges.push(json!({ "bridge": name, "members": members }));
                }
            }
        }
    }
    section(
        "bridges",
        serde_json::to_string_pretty(&bridges).unwrap_or_default(),
    );
    out
}
