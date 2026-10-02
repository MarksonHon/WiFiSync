//! 设备能力探测的**数据模型**（探测动作在 `wifisync-sys`，判断逻辑在这里）。

use serde::{Deserialize, Serialize};

/// 物理网口在出厂拓扑里的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortKind {
    Lan,
    Wan,
    Unknown,
}

impl PortKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PortKind::Lan => "lan",
            PortKind::Wan => "wan",
            PortKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    /// 内核接口名，如 `lan1` / `eth0` / `wan`。
    pub name: String,
    pub kind: PortKind,
    /// DSA 用户端口（DSA 拓扑下加桥是标准做法）。
    pub dsa: bool,
    /// 当前是否有载波（只用于展示，不影响规划）。
    pub carrier: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioInfo {
    /// uci 里的 radio 名，如 `radio0`。
    pub name: String,
    pub band: Option<String>,
    pub channel: Option<u32>,
    /// 该 radio 是否具备 KVR 所需能力（由 wpad 完整版 + 驱动决定）。
    pub supports_kvr: bool,
}

/// 设备能力快照。所有角色/网桥/无线判断的输入。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub model: Option<String>,
    pub board_name: Option<String>,
    pub radios: Vec<RadioInfo>,
    pub ports: Vec<PortInfo>,
    /// 是否安装了完整版 `wpad`（`wpad-basic` 不含 802.11k/v/r）。
    pub wpad_full: bool,
    /// 是否支持 VLAN filtering / VLAN 跨设备同步。
    pub vlan_capable: bool,
}

impl Capabilities {
    /// 需求 3：没有无线模块的设备禁止承担 AP 角色。
    pub fn has_wifi(&self) -> bool {
        !self.radios.is_empty()
    }

    pub fn port_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.ports.iter().map(|p| p.name.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn lan_ports(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .ports
            .iter()
            .filter(|p| p.kind != PortKind::Wan)
            .map(|p| p.name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn wan_ports(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .ports
            .iter()
            .filter(|p| p.kind == PortKind::Wan)
            .map(|p| p.name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn radio_names(&self) -> Vec<String> {
        self.radios.iter().map(|r| r.name.clone()).collect()
    }

    /// 是否具备 KVR 的前置条件。
    pub fn kvr_ready(&self) -> bool {
        self.wpad_full && self.radios.iter().any(|r| r.supports_kvr)
    }

    pub fn summary(&self) -> String {
        format!(
            "board={} radios={} ports={} wpad_full={} vlan={}",
            self.board_name.as_deref().unwrap_or("unknown"),
            self.radios.len(),
            self.ports.len(),
            self.wpad_full,
            self.vlan_capable
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, kind: PortKind) -> PortInfo {
        PortInfo {
            name: name.to_string(),
            kind,
            dsa: true,
            carrier: true,
        }
    }

    #[test]
    fn lan_ports_exclude_wan() {
        let caps = Capabilities {
            ports: vec![
                port("lan1", PortKind::Lan),
                port("lan2", PortKind::Lan),
                port("wan", PortKind::Wan),
            ],
            ..Default::default()
        };
        assert_eq!(caps.lan_ports(), vec!["lan1", "lan2"]);
        assert_eq!(caps.wan_ports(), vec!["wan"]);
        assert_eq!(caps.port_names(), vec!["lan1", "lan2", "wan"]);
    }

    #[test]
    fn no_radios_means_no_wifi() {
        let caps = Capabilities::default();
        assert!(!caps.has_wifi());
        assert!(!caps.kvr_ready());
    }
}
