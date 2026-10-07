//! Read-only collection of the LAN area information a Gateway reports to the Controller.
//!
//! Sources: `/etc/config/network`, `/etc/config/dhcp`, the bridge in sysfs and, when available,
//! the runtime state of the LAN interface from netifd. Nothing here writes anything.

use crate::exec;
use crate::paths::Paths;
use wifisync_core::lan::{LanInput, LanReport};
use wifisync_core::link::valid_identifier;
use wifisync_core::uci_file;

/// Members of a running bridge (`/sys/class/net/<bridge>/brif`).
fn bridge_members(paths: &Paths, bridge: &str) -> Vec<String> {
    let dir = paths.root().join("sys/class/net").join(bridge).join("brif");
    let mut ports: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    ports.sort();
    ports
}

/// `ubus call network.interface.<name> status`, parsed. Only on a real system (`/` root).
fn interface_status(paths: &Paths, interface: &str) -> Option<serde_json::Value> {
    let real_root = paths.root() == std::path::Path::new("/");
    if !real_root || !exec::has("ubus") || !valid_identifier(interface, 32) {
        return None;
    }
    let object = format!("network.interface.{}", interface);
    let out = exec::run("ubus", &["call", &object, "status"]).ok()?;
    if !out.success() {
        return None;
    }
    serde_json::from_str(&out.stdout).ok()
}

/// Build the LAN report of this device.
pub fn collect(paths: &Paths, bridge_name: &str, now: wifisync_core::Timestamp) -> LanReport {
    let read = |pkg: &str| {
        std::fs::read_to_string(paths.uci_file(pkg))
            .map(|text| uci_file::parse(&text))
            .unwrap_or_default()
    };
    let network = read("network");
    let dhcp = read("dhcp");

    let mut report = LanReport::build(&LanInput {
        network: &network,
        dhcp: &dhcp,
        bridge_name,
        runtime_ports: Vec::new(),
        bridge_present: false,
        now,
    });

    // The bridge name may come from the interface definition, so look at sysfs afterwards.
    let bridge = report.bridge.name.clone();
    if valid_identifier(&bridge, 15) {
        let sysfs = paths.root().join("sys/class/net").join(&bridge);
        report.bridge.present = sysfs.join("bridge").exists();
        let members = bridge_members(paths, &bridge);
        if !members.is_empty() {
            report.bridge.ports = members;
        }
    }
    if let Some(status) = interface_status(paths, &report.bridge.interface) {
        report.apply_runtime_status(&status);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wifisync-lan-{}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("etc/config")).unwrap();
        dir
    }

    #[test]
    fn collects_from_config_files_and_sysfs() {
        let root = tmp_root("collect");
        std::fs::write(
            root.join("etc/config/network"),
            "config device\n\toption name 'br-lan'\n\toption type 'bridge'\n\tlist ports 'lan1'\n\nconfig interface 'lan'\n\toption device 'br-lan'\n\toption proto 'static'\n\toption ipaddr '192.168.5.1/24'\n",
        )
        .unwrap();
        std::fs::write(
            root.join("etc/config/dhcp"),
            "config dhcp 'lan'\n\toption interface 'lan'\n\toption start '20'\n\toption limit '30'\n",
        )
        .unwrap();
        let brif = root.join("sys/class/net/br-lan/brif");
        std::fs::create_dir_all(&brif).unwrap();
        std::fs::create_dir_all(root.join("sys/class/net/br-lan/bridge")).unwrap();
        std::fs::create_dir_all(brif.join("lan2")).unwrap();
        std::fs::create_dir_all(brif.join("lan1")).unwrap();

        let report = collect(&Paths::with_root(&root), "br-lan", 7);
        assert!(report.bridge.present);
        assert_eq!(report.bridge.ports, vec!["lan1", "lan2"]);
        assert_eq!(report.ipv4.network.as_deref(), Some("192.168.5.0/24"));
        assert_eq!(report.dhcp.first.as_deref(), Some("192.168.5.20"));
        assert_eq!(report.dhcp.last.as_deref(), Some("192.168.5.49"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_config_yields_an_empty_report() {
        let root = tmp_root("empty");
        let report = collect(&Paths::with_root(&root), "br-lan", 1);
        assert_eq!(report.bridge.name, "br-lan");
        assert!(!report.bridge.present);
        assert!(!report.dhcp.enabled);
        assert!(report.ipv4.address.is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
