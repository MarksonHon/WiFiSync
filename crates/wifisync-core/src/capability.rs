//! **Data model** for device capability probing (the probing itself lives in `wifisync-sys`;
//! the decision logic lives here).

use serde::{Deserialize, Serialize};

/// Role of a physical port in the factory topology.
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
    /// Kernel interface name, e.g. `lan1` / `eth0` / `wan`.
    pub name: String,
    pub kind: PortKind,
    /// DSA user port (adding a bridge is standard practice under a DSA topology).
    pub dsa: bool,
    /// Whether there is currently a carrier (display only; does not affect planning).
    pub carrier: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioInfo {
    /// Radio name in uci, e.g. `radio0`.
    pub name: String,
    pub band: Option<String>,
    pub channel: Option<u32>,
    /// Whether this radio has the capabilities KVR requires (determined by full wpad plus the
    /// driver).
    pub supports_kvr: bool,
}

/// Device capability snapshot. The input to every role/bridge/wireless decision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub model: Option<String>,
    pub board_name: Option<String>,
    pub radios: Vec<RadioInfo>,
    pub ports: Vec<PortInfo>,
    /// Whether full `wpad` is installed (`wpad-basic` lacks 802.11k/v/r).
    pub wpad_full: bool,
    /// Whether VLAN filtering / VLAN cross-device sync is supported.
    pub vlan_capable: bool,
}

impl Capabilities {
    /// Requirement 3: a device without a wireless module must not take the AP role.
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

    /// Whether the prerequisites for KVR are met.
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
