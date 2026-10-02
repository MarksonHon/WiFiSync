//! Bridge planning (requirement 5 / 10).
//!
//! There is only one rule, but it is the **only** one:
//!
//! ```text
//! enable_bridge = roles.ap && !roles.gateway && !roles.controller
//! ```
//!
//! That is: only a "pure AP" device merges all network ports (including the factory WAN port)
//! into `br-lan`; any other role combination returns an empty plan (no port changes, no bridge).
//!
//! The data structure is always a **list** (`Vec<BridgePlan>` with per-bridge `vlans`),
//! by default producing a single `br-lan`, reserving room for future "multi-bridge / VLAN
//! cross-device sync".

use crate::capability::Capabilities;
use crate::message::Message;
use crate::role::Roles;
use serde::{Deserialize, Serialize};

/// VLAN definition inside a bridge (not produced by default in this phase; the structure is
/// fixed up front).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VlanDef {
    pub id: u16,
    pub name: String,
    pub tagged_ports: Vec<String>,
    pub untagged_ports: Vec<String>,
}

impl VlanDef {
    pub fn new(id: u16, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            tagged_ports: Vec::new(),
            untagged_ports: Vec::new(),
        }
    }
}

/// Complete definition of one bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgePlan {
    pub name: String,
    pub ports: Vec<String>,
    pub vlan_filtering: bool,
    pub vlans: Vec<VlanDef>,
}

impl BridgePlan {
    pub fn new(name: impl Into<String>, ports: Vec<String>) -> Self {
        Self {
            name: name.into(),
            ports,
            vlan_filtering: false,
            vlans: Vec::new(),
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "{} <- [{}]{}",
            self.name,
            self.ports.join(", "),
            if self.vlans.is_empty() {
                String::new()
            } else {
                format!(" + {} vlan", self.vlans.len())
            }
        )
    }
}

pub struct BridgePlanInput<'a> {
    pub roles: Roles,
    pub caps: &'a Capabilities,
    /// Default bridge name (`br-lan`).
    pub bridge_name: &'a str,
    /// Extra bridge definitions supplied by the caller (multi-bridge extension point, empty by
    /// default).
    pub extra: Vec<BridgePlan>,
}

/// Plan bridges. Returns an empty Vec unless the device is a "pure AP" — [`crate::plan`]
/// guarantees no writes are produced.
pub fn plan_bridges(input: &BridgePlanInput<'_>) -> Vec<BridgePlan> {
    if !input.roles.enable_bridge() {
        return Vec::new();
    }

    let mut plans = Vec::new();
    let ports = input.caps.port_names();
    if !ports.is_empty() {
        plans.push(BridgePlan::new(input.bridge_name, ports));
    }
    // Multi-bridge / VLAN extension point: injected by the caller, keeping the model and
    // ordering stable.
    plans.extend(input.extra.clone());
    plans
}

/// Explains to the UI why no bridge is created.
///
/// The blockers travel as comma separated tokens (`gateway`, `controller`,
/// `no_ap`, or `none`) so the front end can render the role names in the
/// user's language.
pub fn bridge_disabled_reason(roles: &Roles) -> Option<Message> {
    if roles.enable_bridge() {
        return None;
    }
    let mut blockers = Vec::new();
    if roles.gateway {
        blockers.push("gateway");
    }
    if roles.controller {
        blockers.push("controller");
    }
    if !roles.ap {
        blockers.push("no_ap");
    }
    let blockers = if blockers.is_empty() {
        "none".to_string()
    } else {
        blockers.join(",")
    };
    Some(Message::new("bridge.blocked").param("blockers", blockers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{PortInfo, PortKind};

    fn caps() -> Capabilities {
        let p = |name: &str, kind: PortKind| PortInfo {
            name: name.to_string(),
            kind,
            dsa: true,
            carrier: true,
        };
        Capabilities {
            ports: vec![
                p("lan1", PortKind::Lan),
                p("lan2", PortKind::Lan),
                p("wan", PortKind::Wan),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn pure_ap_bridges_all_ports_including_wan() {
        let caps = caps();
        let plans = plan_bridges(&BridgePlanInput {
            roles: Roles::ap_only(),
            caps: &caps,
            bridge_name: "br-lan",
            extra: vec![],
        });
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].name, "br-lan");
        assert_eq!(plans[0].ports, vec!["lan1", "lan2", "wan"]);
    }

    #[test]
    fn every_other_role_combination_produces_no_bridge() {
        for bits in 0u8..8 {
            let roles = Roles {
                controller: bits & 0b001 != 0,
                ap: bits & 0b010 != 0,
                gateway: bits & 0b100 != 0,
            };
            if roles.enable_bridge() {
                continue;
            }
            let caps = caps();
            let plans = plan_bridges(&BridgePlanInput {
                roles,
                caps: &caps,
                bridge_name: "br-lan",
                extra: vec![],
            });
            assert!(
                plans.is_empty(),
                "roles={:?} must not create a bridge",
                roles
            );
        }
    }

    #[test]
    fn no_ports_means_no_bridge_plan() {
        let caps = Capabilities::default();
        let plans = plan_bridges(&BridgePlanInput {
            roles: Roles::ap_only(),
            caps: &caps,
            bridge_name: "br-lan",
            extra: vec![],
        });
        assert!(plans.is_empty());
    }

    #[test]
    fn multi_bridge_extension_point_is_honoured() {
        let caps = caps();
        let mut extra = BridgePlan::new("br-guest", vec!["lan2".into()]);
        extra.vlan_filtering = true;
        extra.vlans.push(VlanDef::new(10, "iot"));
        let plans = plan_bridges(&BridgePlanInput {
            roles: Roles::ap_only(),
            caps: &caps,
            bridge_name: "br-lan",
            extra: vec![extra],
        });
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[1].name, "br-guest");
        assert_eq!(plans[1].vlans[0].id, 10);
    }

    #[test]
    fn disabled_reason_mentions_blockers() {
        let roles = Roles {
            controller: true,
            ap: true,
            gateway: false,
        };
        let reason = bridge_disabled_reason(&roles).unwrap();
        assert_eq!(reason.key, "bridge.blocked");
        assert_eq!(
            reason.params.get("blockers").map(String::as_str),
            Some("controller")
        );
        assert!(bridge_disabled_reason(&Roles::ap_only()).is_none());
    }
}
