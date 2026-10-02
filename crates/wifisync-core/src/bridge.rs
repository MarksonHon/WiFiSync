//! 网桥规划（需求 5 / 10）。
//!
//! 规则只有一条，但是**唯一**一条：
//!
//! ```text
//! enable_bridge = roles.ap && !roles.gateway && !roles.controller
//! ```
//!
//! 即：只有「纯 AP」设备才把所有网口（含出厂 WAN 口）并入 `br-lan`；
//! 其它任何角色组合一律返回空计划（不改网口、不建桥）。
//!
//! 数据结构一律是 **列表**（`Vec<BridgePlan>` + 每桥 `vlans`），
//! 默认只产出一个 `br-lan`，为将来「多网桥 / VLAN 跨设备同步」预留。

use crate::capability::Capabilities;
use crate::role::Roles;
use serde::{Deserialize, Serialize};

/// 网桥内的 VLAN 定义（本期默认不产出，结构先固定下来）。
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

/// 一个网桥的完整定义。
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
    /// 默认网桥名（`br-lan`）。
    pub bridge_name: &'a str,
    /// 调用方额外提供的网桥定义（多网桥扩展点，默认空）。
    pub extra: Vec<BridgePlan>,
}

/// 规划网桥。非「纯 AP」时返回空 Vec —— 由 [`crate::plan`] 保证不产生任何写入。
pub fn plan_bridges(input: &BridgePlanInput<'_>) -> Vec<BridgePlan> {
    if !input.roles.enable_bridge() {
        return Vec::new();
    }

    let mut plans = Vec::new();
    let ports = input.caps.port_names();
    if !ports.is_empty() {
        plans.push(BridgePlan::new(input.bridge_name, ports));
    }
    // 多网桥 / VLAN 扩展点：由调用方注入，保持模型与顺序稳定。
    plans.extend(input.extra.clone());
    plans
}

/// 供 UI 展示「为什么没有网桥」。
pub fn bridge_disabled_reason(roles: &Roles) -> Option<String> {
    if roles.enable_bridge() {
        return None;
    }
    let mut blockers = Vec::new();
    if roles.gateway {
        blockers.push("Gateway");
    }
    if roles.controller {
        blockers.push("Controller");
    }
    if !roles.ap {
        blockers.push("未承担 AP 角色");
    }
    Some(format!(
        "当前角色组合不建桥、不动网口（{}）",
        if blockers.is_empty() {
            "无".to_string()
        } else {
            blockers.join(" / ")
        }
    ))
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
            assert!(plans.is_empty(), "roles={:?} 不应建桥", roles);
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
        assert!(reason.contains("Controller"));
        assert!(bridge_disabled_reason(&Roles::ap_only()).is_none());
    }
}
