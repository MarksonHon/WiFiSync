//! 只读能力探测：`/sys/class/net`、`/sys/class/ieee80211`、`/etc/board.json`。
//!
//! 这里**只读不写**，任何角色都可以安全调用。

use crate::error::SysResult;
use crate::paths::Paths;
use serde_json::Value;
use std::path::Path;
use wifisync_core::capability::{Capabilities, PortInfo, PortKind, RadioInfo};

/// 读取 `/etc/board.json`（OpenWrt 的设备描述文件）。
pub fn read_board_json(paths: &Paths) -> Option<Value> {
    let path = paths.root().join("etc/board.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 是否有无线硬件。
pub fn has_wireless(paths: &Paths) -> bool {
    let dir = paths.root().join("sys/class/ieee80211");
    std::fs::read_dir(dir)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

/// 枚举无线 phy（`phy0`, `phy1`, ...）。
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

/// 是否为无线接口。
fn is_wireless_iface(net_path: &Path) -> bool {
    net_path.join("phy80211").exists()
}

/// 已经是网桥的接口不应再作为成员端口。
fn is_bridge_iface(net_path: &Path) -> bool {
    net_path.join("bridge").exists()
}

fn read_flag(net_path: &Path, name: &str) -> bool {
    std::fs::read_to_string(net_path.join(name))
        .map(|v| v.trim() == "1")
        .unwrap_or(false)
}

/// 枚举物理网口（排除 lo / 无线 / 已有网桥）。
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
        // 只接受有 device 链接的真实设备（虚拟接口如 tun/ppp 会被排除）
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

/// 从 board.json 推断端口角色（`network.lan.device` / `network.wan.device` / `*.ports`）。
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

/// 读取某个网桥当前的成员（sysfs 的 `brif` 目录）。
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

/// 检测是否安装了**完整版** wpad（`wpad-basic*` 不含 802.11k/v/r）。
///
/// 做法：在二进制里找 `mobility_domain` 字符串 —— 这是 hostapd 完整版的编译期特性，
/// 比解析 opkg/apk 数据库更稳（两种包管理器都适用）。
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

/// 是否支持 VLAN filtering（DSA 设备普遍支持）。
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
    // 有 DSA 端口就当支持
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

/// 从 `/etc/config/wireless` 解析 radio 列表（uci 是权威来源，顺序即 radioN）。
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
            // 具体能力由 wpad + 驱动决定，这里先标记，最终值在 `capabilities()` 里统一修正
            supports_kvr: section.option("disabled") != Some("1"),
        });
    }
    radios
}

/// 探测完整能力快照。
///
/// **无线存在性的权威来源是内核无线子系统**（`/sys/class/ieee80211/*`），
/// 而不是 `/etc/config/wireless`：残留的 uci 配置不能证明设备真的有无线模块
/// （需求 3 要求「没有无线模块就禁止 AP」，这里必须保守）。
/// uci 配置只用来给 phy 补充 radio 名/频段等展示信息。
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
            // 只保留数量与 phy 对得上的那些（多余的是残留配置）
            .take(phys.len())
            .collect()
    };
    if radios.is_empty() && !phys.is_empty() {
        // 没有 uci 配置时退回 sysfs 枚举（数量可靠，名称是 phyN）
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

    /// 需求 3 的关键回归：残留的 uci 无线配置**不能**证明设备有无线模块。
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
            "没有 /sys/class/ieee80211/* 时必须判定为无无线硬件"
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
