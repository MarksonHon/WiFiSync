//! Read-only capability probing: `/sys/class/net`, `/sys/class/ieee80211`, `/etc/board.json`.
//!
//! This module is **read-only, never writes**, so any role can call it safely.

use crate::error::SysResult;
use crate::paths::Paths;
use serde_json::Value;
use std::path::Path;
use wifisync_core::capability::{Capabilities, PortInfo, PortKind, RadioInfo};

/// Read `/etc/board.json` (OpenWrt's device description file).
pub fn read_board_json(paths: &Paths) -> Option<Value> {
    let path = paths.root().join("etc/board.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether wireless hardware is present.
pub fn has_wireless(paths: &Paths) -> bool {
    let dir = paths.root().join("sys/class/ieee80211");
    std::fs::read_dir(dir)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

/// Enumerate wireless phys (`phy0`, `phy1`, ...).
pub fn list_phys(paths: &Paths) -> Vec<String> {
    let dir = paths.root().join("sys/class/ieee80211");
    let mut names: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names
}

/// Whether this is a wireless interface.
fn is_wireless_iface(net_path: &Path) -> bool {
    net_path.join("phy80211").exists()
}

/// Interfaces that are already bridges must not be used as member ports again.
fn is_bridge_iface(net_path: &Path) -> bool {
    net_path.join("bridge").exists()
}

fn read_flag(net_path: &Path, name: &str) -> bool {
    std::fs::read_to_string(net_path.join(name))
        .map(|v| v.trim() == "1")
        .unwrap_or(false)
}

/// Enumerate physical network ports (excluding lo / wireless / existing bridges).
pub fn list_net_ports(paths: &Paths, board: Option<&Value>) -> Vec<PortInfo> {
    let net_dir = paths.root().join("sys/class/net");
    let mut ports = Vec::new();

    let Ok(entries) = std::fs::read_dir(&net_dir) else {
        return ports;
    };

    for entry in entries.flatten() {
        let net_path = entry.path();
        let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        if name == "lo" || name.starts_with("br-") || name.starts_with("wl") {
            continue;
        }
        if is_wireless_iface(&net_path) || is_bridge_iface(&net_path) {
            continue;
        }
        // Only accept real devices with a `device` link (virtual ifaces like tun/ppp are excluded)
        if !net_path.join("device").exists() && !net_path.join("dsa").exists() {
            continue;
        }
        ports.push(PortInfo {
            kind: board
                .map(|b| port_kind_from_board(b, &name))
                .unwrap_or(PortKind::Unknown),
            dsa: net_path.join("dsa").exists() || net_path.join("device").join("dsa").exists(),
            carrier: read_flag(&net_path, "carrier"),
            name,
        });
    }

    ports.sort_by_key(|p| p.name.clone());
    ports
}

/// Infer a port's role from board.json (`network.lan.device` / `network.wan.device` / `*.ports`).
pub fn port_kind_from_board(board: &Value, name: &str) -> PortKind {
    for (key, kind) in [("wan", PortKind::Wan), ("lan", PortKind::Lan)] {
        let Some(node) = board.get("network").and_then(|n| n.get(key)) else {
            continue;
        };
        for field in ["device", "ports"] {
            if let Some(value) = node.get(field) {
                if value_matches(value, name) {
                    return kind;
                }
            }
        }
    }
    PortKind::Unknown
}

fn value_matches(value: &Value, name: &str) -> bool {
    match value {
        Value::String(s) => s.split_whitespace().any(|token| token == name),
        Value::Array(items) => items.iter().any(|item| match item {
            Value::String(s) => s == name,
            _ => false,
        }),
        _ => false,
    }
}

/// Read a bridge's current members (the sysfs `brif` directory).
pub fn bridge_members(paths: &Paths, bridge: &str) -> Vec<String> {
    let dir = paths.root().join("sys/class/net").join(bridge).join("brif");
    let mut members = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                members.push(name.to_string());
            }
        }
    }
    members.sort();
    members
}

/// Detect whether the **full** wpad is installed (`wpad-basic*` lacks 802.11k/v/r).
///
/// Method: look for the `mobility_domain` string in the binaries — it is a compile-time
/// feature of the full hostapd and is more robust than parsing the opkg/apk database
/// (works with both package managers).
pub fn wpad_full(paths: &Paths) -> bool {
    let candidates = [
        paths.root().join("usr/sbin/wpad"),
        paths.root().join("usr/sbin/hostapd"),
    ];
    for candidate in candidates {
        if !candidate.exists() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&candidate) {
            if contains(&bytes, b"mobility_domain") && contains(&bytes, b"ieee80211r") {
                return true;
            }
        }
    }
    false
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Whether VLAN filtering is supported (most DSA devices support it).
pub fn vlan_capable(paths: &Paths, board: Option<&Value>) -> bool {
    if let Some(board) = board {
        if board.get("switch").is_some_and(|s| {
            s.get("switch0")
                .and_then(|v| v.get("enable_vlan"))
                .is_some()
        }) {
            return true;
        }
    }
    // Any DSA port counts as supported
    let net_dir = paths.root().join("sys/class/net");
    if let Ok(entries) = std::fs::read_dir(net_dir) {
        for entry in entries.flatten() {
            if entry.path().join("dsa").exists() {
                return true;
            }
        }
    }
    false
}

/// Parse the radio list from `/etc/config/wireless` (uci is authoritative; order gives radioN).
pub fn radios_from_uci(wireless_text: &str) -> Vec<RadioInfo> {
    let sections = wifisync_core::uci_file::parse(wireless_text);
    let mut radios = Vec::new();
    for section in sections.iter().filter(|s| s.kind == "wifi-device") {
        let channel = section
            .option("channel")
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|c| *c > 0);
        let band = section.option("band").map(|b| b.to_string()).or_else(|| {
            match section.option("hwmode") {
                Some("11a") => Some("5g".to_string()),
                Some("11g") | Some("11b") => Some("2g".to_string()),
                _ => None,
            }
        });
        radios.push(RadioInfo {
            name: section.name.clone(),
            band,
            channel,
            // Actual capabilities depend on wpad + driver; mark a provisional value here
            // and fix it in `capabilities()`.
            supports_kvr: section.option("disabled") != Some("1"),
        });
    }
    radios
}

/// Probe a full capability snapshot.
///
/// **The authoritative source for wireless presence is the kernel wireless subsystem**
/// (`/sys/class/ieee80211/*`), not `/etc/config/wireless`: stale uci config cannot prove the
/// device really has a wireless module (requirement 3 says "no wireless module means no AP",
/// so we must be conservative here). uci config only supplements the phy with display info
/// such as radio name/band.
pub fn capabilities(paths: &Paths, wireless_text: Option<&str>) -> SysResult<Capabilities> {
    let board = read_board_json(paths);
    let phys = list_phys(paths);

    let mut radios: Vec<RadioInfo> = if phys.is_empty() {
        Vec::new()
    } else {
        wireless_text
            .map(radios_from_uci)
            .unwrap_or_default()
            .into_iter()
            // Keep only as many as match the phy count (the extras are stale config)
            .take(phys.len())
            .collect()
    };
    if radios.is_empty() && !phys.is_empty() {
        // Without uci config, fall back to sysfs enumeration (count is reliable, names are phyN)
        radios = phys
            .iter()
            .map(|phy| RadioInfo {
                name: phy.clone(),
                band: None,
                channel: None,
                supports_kvr: true,
            })
            .collect();
    }

    let full = wpad_full(paths);
    if !full {
        for radio in radios.iter_mut() {
            radio.supports_kvr = false;
        }
    }

    let model = board
        .as_ref()
        .and_then(|b| b.get("model"))
        .and_then(|m| m.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let board_name = board
        .as_ref()
        .and_then(|b| b.get("board_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(Capabilities {
        model,
        board_name,
        radios,
        ports: list_net_ports(paths, board.as_ref()),
        wpad_full: full,
        vlan_capable: vlan_capable(paths, board.as_ref()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_radios_from_uci() {
        let text = r#"
config wifi-device 'radio0'
	option type 'mac80211'
	option channel '36'
	option band '5g'

config wifi-device 'radio1'
	option type 'mac80211'
	option hwmode '11g'
	option channel '6'
"#;
        let radios = radios_from_uci(text);
        assert_eq!(radios.len(), 2);
        assert_eq!(radios[0].name, "radio0");
        assert_eq!(radios[0].band.as_deref(), Some("5g"));
        assert_eq!(radios[0].channel, Some(36));
        assert_eq!(radios[1].band.as_deref(), Some("2g"));
    }

    #[test]
    fn board_json_port_kinds() {
        let board: Value = serde_json::json!({
            "network": {
                "lan": { "device": "lan1 lan2", "proto": "static" },
                "wan": { "device": "wan", "proto": "dhcp" }
            }
        });
        assert_eq!(port_kind_from_board(&board, "lan1"), PortKind::Lan);
        assert_eq!(port_kind_from_board(&board, "lan2"), PortKind::Lan);
        assert_eq!(port_kind_from_board(&board, "wan"), PortKind::Wan);
        assert_eq!(port_kind_from_board(&board, "eth9"), PortKind::Unknown);
    }

    #[test]
    fn board_json_array_devices() {
        let board: Value = serde_json::json!({
            "network": { "lan": { "ports": ["lan1", "lan2"] } }
        });
        assert_eq!(port_kind_from_board(&board, "lan2"), PortKind::Lan);
    }

    #[test]
    fn contains_finds_substring() {
        assert!(contains(b"hello mobility_domain world", b"mobility_domain"));
        assert!(!contains(b"hello", b"mobility_domain"));
        assert!(!contains(b"", b"x"));
    }

    #[test]
    fn capabilities_without_wifi_hardware() {
        let tmp = std::env::temp_dir().join("wifisync-test-caps-empty");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("etc")).unwrap();
        let paths = Paths::with_root(&tmp);
        let caps = capabilities(&paths, None).unwrap();
        assert!(!caps.has_wifi());
        assert!(!caps.wpad_full);
        assert!(caps.ports.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Requirement 3 regression: stale uci wireless config must **not** prove wireless hardware.
    #[test]
    fn stale_uci_config_does_not_fake_wireless_hardware() {
        let tmp = std::env::temp_dir().join("wifisync-test-caps-stale");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("etc")).unwrap();
        let paths = Paths::with_root(&tmp);
        let stale = "config wifi-device 'radio0'\n\toption channel '36'\n";
        let caps = capabilities(&paths, Some(stale)).unwrap();
        assert!(
            !caps.has_wifi(),
            "without /sys/class/ieee80211/* the device must be detected as having no wireless hardware"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn physical_radio_without_uci_is_detected() {
        let tmp = std::env::temp_dir().join("wifisync-test-caps-phy");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("sys/class/ieee80211/phy0")).unwrap();
        let paths = Paths::with_root(&tmp);
        let caps = capabilities(&paths, None).unwrap();
        assert!(caps.has_wifi());
        assert_eq!(caps.radios[0].name, "phy0");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
