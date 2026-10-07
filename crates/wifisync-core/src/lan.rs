//! LAN report: what the Gateway tells the Controller about the LAN area.
//!
//! The report covers the bridge that carries the LAN, the IPv4 network, the DHCP range and the
//! IPv6 policy. Everything here is **read-only** information derived from `/etc/config/network`,
//! `/etc/config/dhcp` and (optionally) the runtime state of netifd; building a report never
//! changes the network (requirement 7).
//!
//! The parsing is pure so it can be unit-tested on a host; `wifisync-sys::lan` only gathers the
//! inputs.

use crate::uci_file::UciSection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::Ipv4Addr;

/// Upper bounds applied to reports received from the network.
pub const MAX_PORTS: usize = 32;
pub const MAX_LIST: usize = 16;
pub const MAX_TEXT: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanBridge {
    /// Bridge device name, e.g. `br-lan`.
    pub name: String,
    /// Logical netifd interface that sits on the bridge, e.g. `lan`.
    pub interface: String,
    /// Whether the bridge exists in the running system (sysfs).
    pub present: bool,
    pub ports: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanIpv4 {
    /// netifd protocol: `static`, `dhcp`, ...
    pub proto: String,
    pub address: Option<String>,
    pub netmask: Option<String>,
    pub prefix_len: Option<u8>,
    /// Network address in CIDR form, e.g. `192.168.1.0/24`.
    pub network: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanDhcp {
    /// Whether a DHCPv4 server is enabled on the LAN interface.
    pub enabled: bool,
    pub start: Option<u32>,
    pub limit: Option<u32>,
    /// First and last leasable address, computed from the network and `start`/`limit`.
    pub first: Option<String>,
    pub last: Option<String>,
    pub leasetime: Option<String>,
    /// Raw `dhcp_option` entries (e.g. `6,192.168.1.1`).
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanIpv6 {
    /// Stable summary key: `disabled`, `slaac`, `dhcpv6`, `slaac_dhcpv6`, `dhcpv6_stateful`,
    /// `relay`.
    pub mode: String,
    pub dhcpv6: String,
    pub ra: String,
    pub ra_management: Option<u8>,
    pub ra_slaac: bool,
    pub ndp: Option<String>,
    /// Prefix length handed to the LAN from the delegated prefix (`ip6assign`).
    pub ip6assign: Option<u8>,
    pub ula_prefix: Option<String>,
    /// Protocol of the `wan6` interface when one exists (the IPv6 uplink).
    pub upstream_proto: Option<String>,
    /// Runtime: IPv6 addresses and prefixes currently on the LAN interface.
    pub addresses: Vec<String>,
    pub prefixes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanReport {
    pub bridge: LanBridge,
    pub ipv4: LanIpv4,
    pub dhcp: LanDhcp,
    pub ipv6: LanIpv6,
    pub collected_at: crate::Timestamp,
}

/// Inputs of [`LanReport::build`].
pub struct LanInput<'a> {
    pub network: &'a [UciSection],
    pub dhcp: &'a [UciSection],
    /// Preferred bridge name (from the configuration).
    pub bridge_name: &'a str,
    /// Ports of the running bridge (sysfs `brif`); empty when unknown.
    pub runtime_ports: Vec<String>,
    pub bridge_present: bool,
    pub now: crate::Timestamp,
}

fn first_value(section: &UciSection, name: &str) -> Option<String> {
    section
        .option(name)
        .map(|v| v.to_string())
        .or_else(|| section.list(name).first().map(|v| v.to_string()))
        .filter(|v| !v.is_empty())
}

fn bool_opt(section: Option<&UciSection>, name: &str, default: bool) -> bool {
    section
        .and_then(|s| s.option(name))
        .map(|v| matches!(v, "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

/// Netmask → prefix length (only contiguous masks are valid).
pub fn prefix_from_netmask(mask: Ipv4Addr) -> Option<u8> {
    let bits = u32::from(mask);
    let prefix = bits.leading_ones();
    if prefix + bits.trailing_zeros() == 32 || bits == 0 {
        Some(prefix as u8)
    } else {
        None
    }
}

pub fn netmask_from_prefix(prefix: u8) -> Ipv4Addr {
    if prefix == 0 {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::from(u32::MAX << (32 - u32::from(prefix.min(32))))
    }
}

/// Split uci `ipaddr` (`1.2.3.4` or `1.2.3.4/24`) into an address and an optional prefix.
fn split_ipaddr(value: &str) -> Option<(Ipv4Addr, Option<u8>)> {
    match value.split_once('/') {
        Some((addr, mask)) => {
            let addr: Ipv4Addr = addr.parse().ok()?;
            let prefix = match mask.parse::<u8>() {
                Ok(p) if p <= 32 => p,
                _ => prefix_from_netmask(mask.parse().ok()?)?,
            };
            Some((addr, Some(prefix)))
        }
        None => Some((value.parse().ok()?, None)),
    }
}

/// First and last leasable address of a dnsmasq pool (`start` is an offset into the subnet).
pub fn dhcp_range(
    address: Ipv4Addr,
    prefix: u8,
    start: u32,
    limit: u32,
) -> Option<(Ipv4Addr, Ipv4Addr)> {
    if !(8..=30).contains(&prefix) || limit == 0 {
        return None;
    }
    let mask = u32::MAX << (32 - u32::from(prefix));
    let network = u32::from(address) & mask;
    let broadcast = network | !mask;
    let first = network.checked_add(start)?;
    if first >= broadcast {
        return None;
    }
    let last = first.checked_add(limit - 1)?.min(broadcast - 1);
    Some((Ipv4Addr::from(first), Ipv4Addr::from(last)))
}

fn ipv6_mode(dhcpv6: &str, ra: &str, ra_slaac: bool) -> &'static str {
    let dhcpv6_on = dhcpv6 != "disabled";
    let ra_on = ra != "disabled";
    if dhcpv6 == "relay" || ra == "relay" {
        "relay"
    } else {
        match (ra_on, dhcpv6_on) {
            (false, false) => "disabled",
            (true, false) => "slaac",
            (false, true) => "dhcpv6",
            (true, true) if ra_slaac => "slaac_dhcpv6",
            (true, true) => "dhcpv6_stateful",
        }
    }
}

impl LanReport {
    /// Build a report from the parsed uci files.
    pub fn build(input: &LanInput<'_>) -> Self {
        let interface = pick_lan_interface(input.network, input.bridge_name);

        let bridge_name = interface
            .and_then(|s| first_value(s, "device").or_else(|| first_value(s, "ifname")))
            .filter(|device| !device.starts_with('@'))
            .unwrap_or_else(|| input.bridge_name.to_string());
        let interface_name = interface.map(|s| s.name.clone()).unwrap_or_default();

        let configured_ports = bridge_ports(input.network, &bridge_name, interface);
        let ports = if input.runtime_ports.is_empty() {
            configured_ports
        } else {
            input.runtime_ports.clone()
        };

        let mut ipv4 = LanIpv4 {
            proto: interface
                .and_then(|s| s.option("proto"))
                .unwrap_or("none")
                .to_string(),
            ..LanIpv4::default()
        };
        if let Some(section) = interface {
            let mut prefix = None;
            if let Some((addr, cidr_prefix)) =
                first_value(section, "ipaddr").and_then(|v| split_ipaddr(&v))
            {
                ipv4.address = Some(addr.to_string());
                prefix = cidr_prefix;
            }
            if prefix.is_none() {
                prefix = section
                    .option("netmask")
                    .and_then(|m| m.parse::<Ipv4Addr>().ok())
                    .and_then(prefix_from_netmask);
            }
            set_ipv4_prefix(&mut ipv4, prefix);
        }

        let dhcp_section = pick_dhcp_section(input.dhcp, &interface_name);
        let mut dhcp = LanDhcp {
            enabled: dhcp_section.is_some() && !bool_opt(dhcp_section, "ignore", false),
            ..LanDhcp::default()
        };
        if let Some(section) = dhcp_section {
            dhcp.start = section.option("start").and_then(|v| v.parse().ok());
            dhcp.limit = section.option("limit").and_then(|v| v.parse().ok());
            dhcp.leasetime = first_value(section, "leasetime");
            dhcp.options = section
                .list("dhcp_option")
                .into_iter()
                .map(|v| v.to_string())
                .collect();
        }

        let mut report = Self {
            bridge: LanBridge {
                name: bridge_name,
                interface: interface_name,
                present: input.bridge_present,
                ports,
            },
            ipv4,
            dhcp,
            ipv6: build_ipv6(input.network, dhcp_section, interface),
            collected_at: input.now,
        };
        report.refresh_dhcp_range();
        report
    }

    /// Overlay the runtime state of the LAN interface (`ubus call network.interface.<name>
    /// status`): the effective addresses win over what the configuration says.
    pub fn apply_runtime_status(&mut self, status: &Value) {
        if let Some(proto) = status.get("proto").and_then(|v| v.as_str()) {
            if self.ipv4.proto == "none" {
                self.ipv4.proto = proto.to_string();
            }
        }
        if let Some(entry) = status
            .get("ipv4-address")
            .and_then(|v| v.as_array())
            .and_then(|items| items.first())
        {
            if let Some(address) = entry.get("address").and_then(|v| v.as_str()) {
                self.ipv4.address = Some(address.to_string());
            }
            let mask = entry.get("mask").and_then(|v| v.as_u64());
            set_ipv4_prefix(&mut self.ipv4, mask.filter(|m| *m <= 32).map(|m| m as u8));
        }
        let collect = |key: &str| -> Vec<String> {
            status
                .get(key)
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            let address = item.get("address")?.as_str()?;
                            let mask = item.get("mask")?.as_u64()?;
                            Some(format!("{}/{}", address, mask))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        self.ipv6.addresses = collect("ipv6-address");
        let mut prefixes = collect("ipv6-prefix-assignment");
        if prefixes.is_empty() {
            prefixes = collect("ipv6-prefix");
        }
        self.ipv6.prefixes = prefixes;
        self.refresh_dhcp_range();
    }

    fn refresh_dhcp_range(&mut self) {
        self.dhcp.first = None;
        self.dhcp.last = None;
        if !self.dhcp.enabled {
            return;
        }
        let (Some(address), Some(prefix)) = (
            self.ipv4
                .address
                .as_deref()
                .and_then(|a| a.parse::<Ipv4Addr>().ok()),
            self.ipv4.prefix_len,
        ) else {
            return;
        };
        // dnsmasq defaults used by OpenWrt when the options are absent
        let start = self.dhcp.start.unwrap_or(100);
        let limit = self.dhcp.limit.unwrap_or(150);
        if let Some((first, last)) = dhcp_range(address, prefix, start, limit) {
            self.dhcp.first = Some(first.to_string());
            self.dhcp.last = Some(last.to_string());
        }
    }

    /// Reject reports that are malformed or oversized (they come from the network).
    ///
    /// The bounds also cap what the Controller stores and relays to every AP: a few KiB per
    /// Gateway.
    pub fn validate(&self) -> Result<(), String> {
        let plain = |text: &str| text.len() <= MAX_TEXT && !text.chars().any(|c| c.is_control());
        let opt_plain = |text: &Option<String>| text.as_deref().map_or(true, plain);
        let addr_like = |text: &Option<String>| {
            text.as_deref().map_or(true, |t| {
                plain(t)
                    && t.chars()
                        .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.' | '/'))
            })
        };

        let texts_ok = [
            &self.bridge.name,
            &self.bridge.interface,
            &self.ipv4.proto,
            &self.ipv6.mode,
            &self.ipv6.dhcpv6,
            &self.ipv6.ra,
        ]
        .into_iter()
        .all(|t| plain(t))
            && [
                &self.dhcp.leasetime,
                &self.ipv6.ndp,
                &self.ipv6.upstream_proto,
            ]
            .into_iter()
            .all(opt_plain);
        let addresses_ok = [
            &self.ipv4.address,
            &self.ipv4.netmask,
            &self.ipv4.network,
            &self.dhcp.first,
            &self.dhcp.last,
            &self.ipv6.ula_prefix,
        ]
        .into_iter()
        .all(addr_like);
        if !texts_ok || !addresses_ok {
            return Err("LAN report contains an invalid field".to_string());
        }

        if self.bridge.ports.len() > MAX_PORTS
            || self.dhcp.options.len() > MAX_LIST
            || self.ipv6.addresses.len() > MAX_LIST
            || self.ipv6.prefixes.len() > MAX_LIST
        {
            return Err("LAN report contains an oversized list".to_string());
        }
        let entries_ok = self
            .bridge
            .ports
            .iter()
            .chain(&self.dhcp.options)
            .chain(&self.ipv6.addresses)
            .chain(&self.ipv6.prefixes)
            .all(|t| plain(t));
        if !entries_ok {
            return Err("LAN report contains an invalid entry".to_string());
        }
        Ok(())
    }

    /// Make a locally collected report fit the limits of [`LanReport::validate`], so that an
    /// unusual device (many bridge ports, long option strings) still reports something.
    pub fn clamp(&mut self) {
        fn text(value: &mut String) {
            value.retain(|c| !c.is_control());
            truncate(value);
        }
        fn opt(value: &mut Option<String>) {
            if let Some(inner) = value {
                text(inner);
            }
        }
        fn truncate(value: &mut String) {
            let mut end = value.len().min(MAX_TEXT);
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            value.truncate(end);
        }
        fn list(values: &mut Vec<String>, max: usize) {
            values.truncate(max);
            values.iter_mut().for_each(text);
        }

        text(&mut self.bridge.name);
        text(&mut self.bridge.interface);
        text(&mut self.ipv4.proto);
        text(&mut self.ipv6.mode);
        text(&mut self.ipv6.dhcpv6);
        text(&mut self.ipv6.ra);
        for value in [
            &mut self.dhcp.leasetime,
            &mut self.ipv6.ndp,
            &mut self.ipv6.upstream_proto,
            &mut self.ipv4.address,
            &mut self.ipv4.netmask,
            &mut self.ipv4.network,
            &mut self.dhcp.first,
            &mut self.dhcp.last,
            &mut self.ipv6.ula_prefix,
        ] {
            opt(value);
        }
        list(&mut self.bridge.ports, MAX_PORTS);
        list(&mut self.dhcp.options, MAX_LIST);
        list(&mut self.ipv6.addresses, MAX_LIST);
        list(&mut self.ipv6.prefixes, MAX_LIST);
    }
}

fn set_ipv4_prefix(ipv4: &mut LanIpv4, prefix: Option<u8>) {
    ipv4.prefix_len = prefix;
    ipv4.netmask = prefix.map(|p| netmask_from_prefix(p).to_string());
    ipv4.network = match (
        ipv4.address
            .as_deref()
            .and_then(|a| a.parse::<Ipv4Addr>().ok()),
        prefix,
    ) {
        (Some(address), Some(prefix)) => {
            let network = u32::from(address) & u32::from(netmask_from_prefix(prefix));
            Some(format!("{}/{}", Ipv4Addr::from(network), prefix))
        }
        _ => None,
    };
}

/// The logical interface that carries the LAN: the one on the configured bridge, else `lan`.
fn pick_lan_interface<'a>(network: &'a [UciSection], bridge_name: &str) -> Option<&'a UciSection> {
    let interfaces = || network.iter().filter(|s| s.kind == "interface");
    interfaces()
        .find(|s| first_value(s, "device").as_deref() == Some(bridge_name))
        .or_else(|| interfaces().find(|s| first_value(s, "ifname").as_deref() == Some(bridge_name)))
        .or_else(|| interfaces().find(|s| s.name == "lan"))
}

fn pick_dhcp_section<'a>(dhcp: &'a [UciSection], interface: &str) -> Option<&'a UciSection> {
    if interface.is_empty() {
        return None;
    }
    let sections = || dhcp.iter().filter(|s| s.kind == "dhcp");
    sections()
        .find(|s| s.option("interface") == Some(interface))
        .or_else(|| sections().find(|s| s.name == interface))
}

/// Ports from the `config device` bridge section, or from a legacy bridge interface.
fn bridge_ports(
    network: &[UciSection],
    bridge_name: &str,
    interface: Option<&UciSection>,
) -> Vec<String> {
    let device = network.iter().find(|s| {
        s.kind == "device"
            && s.option("name") == Some(bridge_name)
            && s.option("type") == Some("bridge")
    });
    if let Some(device) = device {
        return device.list("ports").into_iter().map(String::from).collect();
    }
    interface
        .filter(|s| s.option("type") == Some("bridge"))
        .map(|s| {
            let mut ports: Vec<String> = s.list("ifname").into_iter().map(String::from).collect();
            if ports.is_empty() {
                if let Some(single) = s.option("ifname") {
                    ports = single.split_whitespace().map(String::from).collect();
                }
            }
            ports
        })
        .unwrap_or_default()
}

fn build_ipv6(
    network: &[UciSection],
    dhcp: Option<&UciSection>,
    interface: Option<&UciSection>,
) -> LanIpv6 {
    let option = |name: &str| dhcp.and_then(|s| first_value(s, name));
    let dhcpv6 = option("dhcpv6").unwrap_or_else(|| "disabled".to_string());
    let ra = option("ra").unwrap_or_else(|| "disabled".to_string());
    let ra_slaac = bool_opt(dhcp, "ra_slaac", true);

    LanIpv6 {
        mode: ipv6_mode(&dhcpv6, &ra, ra_slaac).to_string(),
        ra_management: option("ra_management").and_then(|v| v.parse().ok()),
        ndp: option("ndp"),
        ip6assign: interface
            .and_then(|s| s.option("ip6assign"))
            .and_then(|v| v.parse().ok()),
        ula_prefix: network
            .iter()
            .find(|s| s.kind == "globals")
            .and_then(|s| first_value(s, "ula_prefix")),
        upstream_proto: network
            .iter()
            .find(|s| s.kind == "interface" && s.name == "wan6")
            .and_then(|s| first_value(s, "proto")),
        dhcpv6,
        ra,
        ra_slaac,
        addresses: Vec::new(),
        prefixes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uci_file;

    const NETWORK: &str = r#"
config globals 'globals'
	option ula_prefix 'fd12:3456:789a::/48'

config device
	option name 'br-lan'
	option type 'bridge'
	list ports 'lan1'
	list ports 'lan2'

config interface 'lan'
	option device 'br-lan'
	option proto 'static'
	option ipaddr '192.168.8.1'
	option netmask '255.255.255.0'
	option ip6assign '60'

config interface 'wan6'
	option proto 'dhcpv6'
"#;

    const DHCP: &str = r#"
config dhcp 'lan'
	option interface 'lan'
	option start '50'
	option limit '100'
	option leasetime '12h'
	option dhcpv6 'server'
	option ra 'server'
	option ra_slaac '1'
	option ra_management '1'
	option ndp 'relay'
	list dhcp_option '6,192.168.8.1'
"#;

    fn build(network: &str, dhcp: &str, bridge: &str) -> LanReport {
        let network = uci_file::parse(network);
        let dhcp = uci_file::parse(dhcp);
        LanReport::build(&LanInput {
            network: &network,
            dhcp: &dhcp,
            bridge_name: bridge,
            runtime_ports: Vec::new(),
            bridge_present: true,
            now: 42,
        })
    }

    #[test]
    fn reports_bridge_network_dhcp_and_ipv6() {
        let report = build(NETWORK, DHCP, "br-lan");
        assert_eq!(report.bridge.name, "br-lan");
        assert_eq!(report.bridge.interface, "lan");
        assert_eq!(report.bridge.ports, vec!["lan1", "lan2"]);
        assert_eq!(report.ipv4.address.as_deref(), Some("192.168.8.1"));
        assert_eq!(report.ipv4.prefix_len, Some(24));
        assert_eq!(report.ipv4.network.as_deref(), Some("192.168.8.0/24"));
        assert!(report.dhcp.enabled);
        assert_eq!(report.dhcp.first.as_deref(), Some("192.168.8.50"));
        assert_eq!(report.dhcp.last.as_deref(), Some("192.168.8.149"));
        assert_eq!(report.dhcp.leasetime.as_deref(), Some("12h"));
        assert_eq!(report.dhcp.options, vec!["6,192.168.8.1"]);
        assert_eq!(report.ipv6.mode, "slaac_dhcpv6");
        assert_eq!(report.ipv6.ip6assign, Some(60));
        assert_eq!(
            report.ipv6.ula_prefix.as_deref(),
            Some("fd12:3456:789a::/48")
        );
        assert_eq!(report.ipv6.upstream_proto.as_deref(), Some("dhcpv6"));
        assert_eq!(report.ipv6.ndp.as_deref(), Some("relay"));
        assert_eq!(report.collected_at, 42);
    }

    #[test]
    fn cidr_ipaddr_and_default_pool() {
        let network = "config interface 'lan'\n\toption device 'br-lan'\n\toption proto 'static'\n\toption ipaddr '10.0.0.1/16'\n";
        let dhcp = "config dhcp 'lan'\n\toption interface 'lan'\n";
        let report = build(network, dhcp, "br-lan");
        assert_eq!(report.ipv4.prefix_len, Some(16));
        assert_eq!(report.ipv4.netmask.as_deref(), Some("255.255.0.0"));
        assert_eq!(report.dhcp.first.as_deref(), Some("10.0.0.100"));
        assert_eq!(report.dhcp.last.as_deref(), Some("10.0.0.249"));
        assert_eq!(report.ipv6.mode, "disabled");
    }

    #[test]
    fn dhcp_ignore_disables_the_server() {
        let dhcp = "config dhcp 'lan'\n\toption interface 'lan'\n\toption ignore '1'\n";
        let report = build(NETWORK, dhcp, "br-lan");
        assert!(!report.dhcp.enabled);
        assert!(report.dhcp.first.is_none());
    }

    #[test]
    fn missing_dhcp_section_means_disabled() {
        let report = build(NETWORK, "", "br-lan");
        assert!(!report.dhcp.enabled);
    }

    #[test]
    fn custom_bridge_name_follows_the_interface_device() {
        let network = "config device\n\toption name 'br-home'\n\toption type 'bridge'\n\tlist ports 'eth1'\n\nconfig interface 'lan'\n\toption device 'br-home'\n\toption proto 'static'\n\toption ipaddr '172.16.0.1/24'\n";
        let report = build(network, "", "br-lan");
        assert_eq!(report.bridge.name, "br-home");
        assert_eq!(report.bridge.ports, vec!["eth1"]);
    }

    #[test]
    fn runtime_ports_win_over_configured_ones() {
        let network = uci_file::parse(NETWORK);
        let report = LanReport::build(&LanInput {
            network: &network,
            dhcp: &[],
            bridge_name: "br-lan",
            runtime_ports: vec!["lan1".into(), "lan3".into(), "wlan0".into()],
            bridge_present: true,
            now: 0,
        });
        assert_eq!(report.bridge.ports, vec!["lan1", "lan3", "wlan0"]);
    }

    #[test]
    fn runtime_status_overrides_dhcp_client_lan() {
        let network = "config interface 'lan'\n\toption device 'br-lan'\n\toption proto 'dhcp'\n";
        let dhcp = "config dhcp 'lan'\n\toption interface 'lan'\n\toption start '10'\n\toption limit '20'\n";
        let mut report = build(network, dhcp, "br-lan");
        assert!(report.ipv4.address.is_none());
        report.apply_runtime_status(&serde_json::json!({
            "proto": "dhcp",
            "ipv4-address": [{ "address": "192.168.50.2", "mask": 24 }],
            "ipv6-address": [{ "address": "fd00::1", "mask": 64 }],
            "ipv6-prefix-assignment": [{ "address": "2001:db8:1::", "mask": 64 }],
        }));
        assert_eq!(report.ipv4.address.as_deref(), Some("192.168.50.2"));
        assert_eq!(report.ipv4.network.as_deref(), Some("192.168.50.0/24"));
        assert_eq!(report.dhcp.first.as_deref(), Some("192.168.50.10"));
        assert_eq!(report.dhcp.last.as_deref(), Some("192.168.50.29"));
        assert_eq!(report.ipv6.addresses, vec!["fd00::1/64"]);
        assert_eq!(report.ipv6.prefixes, vec!["2001:db8:1::/64"]);
    }

    #[test]
    fn ipv6_modes() {
        assert_eq!(ipv6_mode("disabled", "disabled", true), "disabled");
        assert_eq!(ipv6_mode("disabled", "server", true), "slaac");
        assert_eq!(ipv6_mode("server", "disabled", true), "dhcpv6");
        assert_eq!(ipv6_mode("server", "server", true), "slaac_dhcpv6");
        assert_eq!(ipv6_mode("server", "server", false), "dhcpv6_stateful");
        assert_eq!(ipv6_mode("relay", "relay", true), "relay");
    }

    #[test]
    fn netmask_helpers() {
        assert_eq!(
            prefix_from_netmask("255.255.255.0".parse().unwrap()),
            Some(24)
        );
        assert_eq!(prefix_from_netmask("255.0.255.0".parse().unwrap()), None);
        assert_eq!(netmask_from_prefix(23).to_string(), "255.255.254.0");
    }

    #[test]
    fn dhcp_range_is_clamped_to_the_subnet() {
        let (first, last) = dhcp_range("192.168.1.1".parse().unwrap(), 24, 200, 500).unwrap();
        assert_eq!(first.to_string(), "192.168.1.200");
        assert_eq!(last.to_string(), "192.168.1.254");
        assert!(dhcp_range("192.168.1.1".parse().unwrap(), 24, 300, 10).is_none());
        assert!(dhcp_range("192.168.1.1".parse().unwrap(), 31, 1, 1).is_none());
    }

    #[test]
    fn oversized_reports_are_rejected() {
        let mut report = LanReport::default();
        assert!(report.validate().is_ok());
        report.bridge.ports = (0..=MAX_PORTS).map(|i| format!("p{}", i)).collect();
        assert!(report.validate().is_err());
    }

    #[test]
    fn every_text_field_is_bounded_and_free_of_control_characters() {
        let long = "a".repeat(MAX_TEXT + 1);
        let mut report = LanReport::default();
        report.ipv6.ula_prefix = Some(long.clone());
        assert!(report.validate().is_err());

        let mut report = LanReport::default();
        report.dhcp.leasetime = Some(long);
        assert!(report.validate().is_err());

        let mut report = LanReport::default();
        report.bridge.name = "br\nlan".into();
        assert!(report.validate().is_err());

        let mut report = LanReport::default();
        report.ipv4.address = Some("<script>".into());
        assert!(report.validate().is_err());
    }

    #[test]
    fn clamping_makes_any_local_report_valid() {
        let mut report = LanReport::default();
        report.bridge.name = "x".repeat(500);
        report.bridge.ports = (0..100)
            .map(|i| format!("port-{}", "é".repeat(i)))
            .collect();
        report.dhcp.options = vec!["6,1.1.1.1\n".repeat(20); 40];
        report.ipv6.ula_prefix = Some("f".repeat(300));
        assert!(report.validate().is_err());
        report.clamp();
        assert!(report.validate().is_ok());
        assert_eq!(report.bridge.ports.len(), MAX_PORTS);
    }
}
