//! iwinfo 输出采集（**只读**）：给 LuCI 展示用，不参与任何写入决策。

use crate::error::SysResult;
use crate::exec;
use crate::paths::Paths;
use serde_json::{json, Value};

/// 本机无线概览（有 iwinfo 就用，没有就退回 sysfs 计数）。
pub fn radio_status(paths: &Paths) -> SysResult<Value> {
    let phy_count = crate::sysfs::list_phys(paths).len();
    if !exec::has("iwinfo") {
        return Ok(json!({
            "available": false,
            "phy_count": phy_count,
            "devices": [],
            "note": "未安装 iwinfo，仅能给出 phy 数量",
        }));
    }

    let mut devices = Vec::new();
    if let Ok(out) = exec::run("iwinfo", &[]) {
        for line in out.stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // 形如: wlan0     ESSID: "Home" / 形如: phy0 ...
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

/// 采集只读状态快照（写入备份目录，用于事后排查）。
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
