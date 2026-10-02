//! 写入计划（需求 5 / 7 / 8 / 9 的收口处）。
//!
//! 整个程序**只通过这一份计划**去修改系统。因此只要证明：
//!
//! * `roles.enable_bridge() == false` ⇒ 不产生网桥写入；
//! * `roles.ap == false` ⇒ 不产生任何无线写入；
//!
//! 就能证明「Gateway / Controller 零侵入」。这两条在单测里被穷举验证。

use crate::bridge::{bridge_disabled_reason, plan_bridges, BridgePlan, BridgePlanInput};
use crate::capability::Capabilities;
use crate::profile::NetworkProfile;
use crate::role::Roles;
use crate::wifi_source::ResolvedWifi;
use serde::{Deserialize, Serialize};

/// uci 写操作类型。删除是必须的：Baseline 中不存在的键在恢复时要删掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UciOpKind {
    /// `uci set file.section.option=value`
    Set,
    /// `uci delete file.section.option`
    Delete,
    /// `uci add_list file.section.option=value`
    AddList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UciOp {
    pub kind: UciOpKind,
    /// uci 配置文件（不含路径与扩展名），如 `network` / `wireless`。
    pub file: String,
    /// section 名或 `@type[-n]`。
    pub section: String,
    /// `None` 表示整个 section。
    pub option: Option<String>,
    pub value: Option<String>,
}

impl UciOp {
    pub fn set(
        file: impl Into<String>,
        section: impl Into<String>,
        option: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            kind: UciOpKind::Set,
            file: file.into(),
            section: section.into(),
            option: Some(option.into()),
            value: Some(value.into()),
        }
    }

    pub fn add_list(
        file: impl Into<String>,
        section: impl Into<String>,
        option: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            kind: UciOpKind::AddList,
            file: file.into(),
            section: section.into(),
            option: Some(option.into()),
            value: Some(value.into()),
        }
    }

    pub fn delete(
        file: impl Into<String>,
        section: impl Into<String>,
        option: impl Into<String>,
    ) -> Self {
        Self {
            kind: UciOpKind::Delete,
            file: file.into(),
            section: section.into(),
            option: Some(option.into()),
            value: None,
        }
    }

    /// `network.lan.ipaddr` 形式的键（用于 `managed_keys` 与恢复）。
    pub fn key(&self) -> String {
        match &self.option {
            Some(option) => format!("{}.{}.{}", self.file, self.section, option),
            None => format!("{}.{}", self.file, self.section),
        }
    }

    /// 用于 dry-run 展示的一行。
    pub fn describe(&self) -> String {
        match self.kind {
            UciOpKind::Set => format!(
                "uci set {}.{}.{}='{}'",
                self.file,
                self.section,
                self.option.as_deref().unwrap_or(""),
                self.value.as_deref().unwrap_or("")
            ),
            UciOpKind::AddList => format!(
                "uci add_list {}.{}.{}='{}'",
                self.file,
                self.section,
                self.option.as_deref().unwrap_or(""),
                self.value.as_deref().unwrap_or("")
            ),
            UciOpKind::Delete => format!(
                "uci delete {}.{}.{}",
                self.file,
                self.section,
                self.option.as_deref().unwrap_or("")
            ),
        }
    }
}

/// 一份完整的写入计划。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritePlan {
    pub ops: Vec<UciOp>,
    pub reload_network: bool,
    pub reload_wifi: bool,
    /// 给用户看的说明与告警。
    pub notes: Vec<String>,
    /// 计划涉及的网桥（列表，支持多网桥）。
    pub bridges: Vec<BridgePlan>,
    /// Wi-Fi 写入目标 radio（空 = 不写无线）。
    pub wifi_radios: Vec<String>,
}

impl WritePlan {
    /// 唯一的「零侵入」判据。
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn managed_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.ops.iter().map(|op| op.key()).collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// dry-run 文本（LuCI / CLI 直接展示）。
    pub fn dry_run_text(&self) -> String {
        if self.is_empty() {
            return "本角色组合不会修改任何网络配置（0 项改动）".to_string();
        }
        let mut out = String::new();
        for note in &self.notes {
            out.push_str(&format!("# {}\n", note));
        }
        for op in &self.ops {
            out.push_str(&op.describe());
            out.push('\n');
        }
        if self.reload_network {
            out.push_str("ubus call network reload\n");
        }
        if self.reload_wifi {
            out.push_str("/sbin/wifi reload\n");
        }
        out
    }

    pub fn summary(&self) -> String {
        if self.is_empty() {
            return "无改动".to_string();
        }
        format!(
            "{} 项 uci 改动 / {} 个网桥 / {} 个 radio",
            self.ops.len(),
            self.bridges.len(),
            self.wifi_radios.len()
        )
    }
}

pub struct PlanInput<'a> {
    pub roles: Roles,
    pub caps: &'a Capabilities,
    pub bridge_name: &'a str,
    /// 多网桥扩展点。
    pub extra_bridges: Vec<BridgePlan>,
    /// 已解析的 Wi-Fi 信息（可能为 `None`）。
    pub wifi: Option<&'a ResolvedWifi>,
    /// 可选档案（用于网桥/VLAN 的后续扩展，目前只读取 notes）。
    pub profile: Option<&'a NetworkProfile>,
}

/// 生成写入计划 —— 全程序唯一的网络写入来源。
pub fn build_write_plan(input: &PlanInput<'_>) -> WritePlan {
    let mut plan = WritePlan::default();

    // ── 1. 网桥：只有「纯 AP」才建桥（需求 5）──────────────────────────────
    let bridges = plan_bridges(&BridgePlanInput {
        roles: input.roles,
        caps: input.caps,
        bridge_name: input.bridge_name,
        extra: input.extra_bridges.clone(),
    });
    if bridges.is_empty() {
        if let Some(reason) = bridge_disabled_reason(&input.roles) {
            plan.notes.push(reason);
        }
    } else {
        for bridge in &bridges {
            plan.notes.push(format!("网桥规划：{}", bridge.summary()));
            // `config device` 段：DSA/bridge 统一写法
            plan.ops
                .push(UciOp::set("network", &bridge.name, "name", &bridge.name));
            plan.ops
                .push(UciOp::set("network", &bridge.name, "type", "bridge"));
            if bridge.vlan_filtering {
                plan.ops
                    .push(UciOp::set("network", &bridge.name, "vlan_filtering", "1"));
            }
            for port in &bridge.ports {
                plan.ops
                    .push(UciOp::add_list("network", &bridge.name, "ports", port));
            }
            for vlan in &bridge.vlans {
                let section = format!("{}_{}", bridge.name, vlan.id);
                plan.ops.push(UciOp::set(
                    "network",
                    &section,
                    "name",
                    format!("{}.{}", bridge.name, vlan.id),
                ));
                plan.ops
                    .push(UciOp::set("network", &section, "type", "bridge-vlan"));
                plan.ops
                    .push(UciOp::set("network", &section, "vlan", vlan.id.to_string()));
                for port in &vlan.untagged_ports {
                    plan.ops.push(UciOp::add_list(
                        "network",
                        &section,
                        "ports",
                        format!("{}:u*", port),
                    ));
                }
                for port in &vlan.tagged_ports {
                    plan.ops.push(UciOp::add_list(
                        "network",
                        &section,
                        "ports",
                        format!("{}:t", port),
                    ));
                }
            }
        }
        plan.reload_network = true;
        plan.bridges = bridges;
    }

    // ── 2. 无线：只有承担 AP 角色的设备才写（需求 7 / 8 / 9）───────────────
    if input.roles.ap {
        if let Some(resolved) = input.wifi {
            let wifi = &resolved.profile;
            let radio = wifi.radio.clone();
            plan.notes.extend(resolved.notes.clone());
            if wifi.kvr.r || wifi.kvr.k || wifi.kvr.v {
                plan.notes.push(format!(
                    "KVR: k={} v={} r={} mobility_domain={} ft_over_ds={}",
                    wifi.kvr.k,
                    wifi.kvr.v,
                    wifi.kvr.r,
                    wifi.kvr.mobility_domain,
                    wifi.kvr.ft_over_ds
                ));
            }
            let wifi_section = format!("wifisync_{}", radio);
            plan.ops
                .push(UciOp::set("wireless", &wifi_section, "device", &radio));
            plan.ops
                .push(UciOp::set("wireless", &wifi_section, "mode", "ap"));
            plan.ops
                .push(UciOp::set("wireless", &wifi_section, "network", "lan"));
            plan.ops
                .push(UciOp::set("wireless", &wifi_section, "ssid", &wifi.ssid));
            plan.ops.push(UciOp::set(
                "wireless",
                &wifi_section,
                "encryption",
                &wifi.auth,
            ));
            if wifi.auth != "none" {
                plan.ops.push(UciOp::set(
                    "wireless",
                    &wifi_section,
                    "key_ref",
                    &wifi.psk_ref,
                ));
            }
            plan.ops.push(UciOp::set(
                "wireless",
                &wifi_section,
                "ieee80211k",
                bool01(wifi.kvr.k),
            ));
            plan.ops.push(UciOp::set(
                "wireless",
                &wifi_section,
                "ieee80211v",
                bool01(wifi.kvr.v),
            ));
            plan.ops.push(UciOp::set(
                "wireless",
                &wifi_section,
                "ieee80211r",
                bool01(wifi.kvr.r),
            ));
            if wifi.kvr.r {
                plan.ops.push(UciOp::set(
                    "wireless",
                    &wifi_section,
                    "mobility_domain",
                    &wifi.kvr.mobility_domain,
                ));
                plan.ops.push(UciOp::set(
                    "wireless",
                    &wifi_section,
                    "ft_over_ds",
                    bool01(wifi.kvr.ft_over_ds),
                ));
            }
            if let Some(channel) = wifi.channel {
                plan.ops.push(UciOp::set(
                    "wireless",
                    &radio,
                    "channel",
                    channel.to_string(),
                ));
            }
            plan.wifi_radios.push(radio);
            plan.reload_wifi = true;
        }
    } else if input.wifi.is_some() {
        plan.notes.push(
            "本设备不承担 AP 角色，Wi-Fi 信息仅用于下发给 AP，不修改本机无线配置".to_string(),
        );
    }

    if let Some(profile) = input.profile {
        if !profile.is_empty() {
            plan.notes
                .push(format!("参考下发档案 {}", profile.summary()));
        }
    }

    plan
}

fn bool01(value: bool) -> &'static str {
    if value {
        "1"
    } else {
        "0"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{PortInfo, PortKind, RadioInfo};
    use crate::profile::WifiProfile;
    use crate::wifi_source::{ResolvedWifi, WifiSourceKind};

    fn caps() -> Capabilities {
        Capabilities {
            board_name: Some("test-board".into()),
            radios: vec![RadioInfo {
                name: "radio0".into(),
                band: Some("5g".into()),
                channel: Some(36),
                supports_kvr: true,
            }],
            ports: vec![
                PortInfo {
                    name: "lan1".into(),
                    kind: PortKind::Lan,
                    dsa: true,
                    carrier: true,
                },
                PortInfo {
                    name: "wan".into(),
                    kind: PortKind::Wan,
                    dsa: true,
                    carrier: true,
                },
            ],
            wpad_full: true,
            vlan_capable: true,
            ..Default::default()
        }
    }

    fn resolved() -> ResolvedWifi {
        ResolvedWifi {
            profile: WifiProfile::template("radio0"),
            will_modify_local: true,
            requires_confirmation: false,
            source: WifiSourceKind::Custom,
            notes: vec![],
        }
    }

    fn input<'a>(
        roles: Roles,
        caps: &'a Capabilities,
        wifi: Option<&'a ResolvedWifi>,
    ) -> PlanInput<'a> {
        PlanInput {
            roles,
            caps,
            bridge_name: crate::DEFAULT_BRIDGE,
            extra_bridges: Vec::new(),
            wifi,
            profile: None,
        }
    }

    #[test]
    fn pure_ap_writes_bridge_and_wifi() {
        let caps = caps();
        let wifi = resolved();
        let plan = build_write_plan(&input(Roles::ap_only(), &caps, Some(&wifi)));
        assert!(!plan.is_empty());
        assert!(plan.reload_network);
        assert!(plan.reload_wifi);
        assert!(plan
            .ops
            .iter()
            .any(|op| op.file == "network" && op.option.as_deref() == Some("ports")));
        assert!(plan.ops.iter().any(|op| op.file == "wireless"
            && op.option.as_deref() == Some("ieee80211r")
            && op.value.as_deref() == Some("1")));
        assert_eq!(plan.bridges[0].ports, vec!["lan1", "wan"]);
    }

    /// 核心不变量：Gateway / Controller（无论是否带 AP）都不会单独写网桥，
    /// 且完全不承担 AP 的配置连无线都不写。
    #[test]
    fn gateway_and_controller_are_non_invasive() {
        let caps = caps();
        let wifi = resolved();
        for bits in 0u8..8 {
            let roles = Roles {
                controller: bits & 0b001 != 0,
                ap: bits & 0b010 != 0,
                gateway: bits & 0b100 != 0,
            };
            let plan = build_write_plan(&input(roles, &caps, Some(&wifi)));

            if !roles.ap {
                assert!(
                    plan.is_empty(),
                    "不承担 AP 的角色组合必须零写入，roles={:?} plan={:?}",
                    roles,
                    plan
                );
            } else if !roles.enable_bridge() {
                // 带 AP 但不能建桥（如 controller+ap）：允许写无线，但不得写网桥
                assert!(!plan.reload_network, "roles={:?} 不应重载网络", roles);
                assert!(plan.bridges.is_empty(), "roles={:?} 不应有网桥", roles);
                assert!(
                    !plan
                        .ops
                        .iter()
                        .any(|op| op.file == "network" && op.option.as_deref() == Some("ports")),
                    "roles={:?} 不应写网桥端口",
                    roles
                );
            }
        }
    }

    #[test]
    fn managed_keys_are_stable() {
        let caps = caps();
        let plan = build_write_plan(&input(Roles::ap_only(), &caps, None));
        let keys = plan.managed_keys();
        assert!(keys.contains(&"network.br-lan.type".to_string()));
        assert!(keys.contains(&"network.br-lan.ports".to_string()));
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn dry_run_text_explains_no_changes() {
        let caps = caps();
        let plan = build_write_plan(&input(
            Roles {
                controller: true,
                ap: false,
                gateway: true,
            },
            &caps,
            None,
        ));
        assert!(plan.is_empty());
        assert!(plan.dry_run_text().contains("0 项改动"));
    }

    #[test]
    fn wifi_is_not_written_when_ap_absent_even_with_source() {
        let caps = caps();
        let wifi = resolved();
        let plan = build_write_plan(&input(Roles::controller_only(), &caps, Some(&wifi)));
        assert!(plan.is_empty());
        assert!(plan.wifi_radios.is_empty());
        assert!(plan.notes.iter().any(|n| n.contains("不修改本机无线配置")));
    }
}
