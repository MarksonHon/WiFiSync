//! Write plan (where requirements 5 / 7 / 8 / 9 converge).
//!
//! The whole program modifies the system **only through this single plan**. So it is enough to
//! prove:
//!
//! * `roles.enable_bridge() == false` ⇒ no bridge writes are produced;
//! * `roles.ap == false` ⇒ no wireless writes are produced;
//!
//! and that proves "Gateway / Controller are zero-intrusion". Both are exhaustively verified in
//! unit tests.

use crate::bridge::{bridge_disabled_reason, plan_bridges, BridgePlan, BridgePlanInput};
use crate::capability::Capabilities;
use crate::message::Message;
use crate::profile::NetworkProfile;
use crate::role::Roles;
use crate::wifi_source::ResolvedWifi;
use serde::{Deserialize, Serialize};

/// uci write operation kind. Delete is required: keys absent from the Baseline must be removed on
/// restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UciOpKind {
    /// `uci set file.section.option=value`
    Set,
    /// `uci delete file.section.option`
    Delete,
    /// `uci add_list file.section.option=value`
    AddList,
    /// `uci set file.section.option=<secret>`: the stored value is a **secret-store reference**,
    /// substituted at apply time. It is never rendered by [`UciOp::describe`], so the dry-run
    /// text, the logs and the UI stay free of key material.
    SetSecret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UciOp {
    pub kind: UciOpKind,
    /// uci configuration file (without path and extension), e.g. `network` / `wireless`.
    pub file: String,
    /// Section name or `@type[-n]`.
    pub section: String,
    /// `None` means the whole section.
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

    /// Write a value taken from the secret store. The stored value is the **reference**, not the
    /// secret itself: the caller substitutes the real value immediately before applying.
    pub fn set_secret(
        file: impl Into<String>,
        section: impl Into<String>,
        option: impl Into<String>,
        reference: impl Into<String>,
    ) -> Self {
        Self {
            kind: UciOpKind::SetSecret,
            file: file.into(),
            section: section.into(),
            option: Some(option.into()),
            value: Some(reference.into()),
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

    /// Key in `network.lan.ipaddr` form (used for `managed_keys` and restore).
    pub fn key(&self) -> String {
        match &self.option {
            Some(option) => format!("{}.{}.{}", self.file, self.section, option),
            None => format!("{}.{}", self.file, self.section),
        }
    }

    /// One line for dry-run display.
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
            // Never the value: the dry-run text and the UI must not learn a secret.
            UciOpKind::SetSecret => format!(
                "uci set {}.{}.{}='<secret:{}>'",
                self.file,
                self.section,
                self.option.as_deref().unwrap_or(""),
                self.value.as_deref().unwrap_or("")
            ),
        }
    }
}

/// A complete write plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritePlan {
    pub ops: Vec<UciOp>,
    pub reload_network: bool,
    pub reload_wifi: bool,
    /// Explanations and warnings for the UI (translated by the front end).
    pub notes: Vec<Message>,
    /// Bridges involved in the plan (a list, supporting multiple bridges).
    pub bridges: Vec<BridgePlan>,
    /// Target radios for Wi-Fi writes (empty = no wireless writes).
    pub wifi_radios: Vec<String>,
    /// Reasons that make the plan **refuse to apply** (a missing or unusable Wi-Fi key, for
    /// instance): `apply` then refuses outright instead of writing a half-configured network.
    pub blocked: Vec<Message>,
}

impl WritePlan {
    /// The single "zero intrusion" criterion.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Whether the plan must not be applied as it stands.
    pub fn is_blocked(&self) -> bool {
        !self.blocked.is_empty()
    }

    /// Record a reason that prevents applying this plan.
    pub fn block(&mut self, reason: Message) {
        self.blocked.push(reason);
    }

    pub fn managed_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.ops.iter().map(|op| op.key()).collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Language neutral dry-run text: only the uci commands and reload calls.
    ///
    /// Notes are delivered separately as [`Message`] values so the front end
    /// can translate them.
    pub fn dry_run_text(&self) -> String {
        let mut out = String::new();
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
            return "no changes".to_string();
        }
        format!(
            "{} uci change(s) / {} bridge(s) / {} radio(s)",
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
    /// Multi-bridge extension point.
    pub extra_bridges: Vec<BridgePlan>,
    /// Resolved Wi-Fi information (may be `None`).
    pub wifi: Option<&'a ResolvedWifi>,
    /// Optional profile (for later bridge/VLAN extensions; only `notes` is read for now).
    pub profile: Option<&'a NetworkProfile>,
    /// The Wi-Fi key looked up from the secret store, when one is configured. `None` means the
    /// secret is absent, in which case an encrypted network blocks the plan
    /// (see [`crate::wifi_key`]).
    pub wifi_key: Option<&'a str>,
}

/// Build the write plan — the program's only source of network writes.
pub fn build_write_plan(input: &PlanInput<'_>) -> WritePlan {
    let mut plan = WritePlan::default();

    // ── 1. bridges: only a "pure AP" creates a bridge (requirement 5)──────────────────────────────
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
            plan.notes
                .push(Message::new("plan.bridge_planned").param("bridge", bridge.summary()));
            // `config device` section: the unified DSA/bridge form
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

    // ── 2. wireless: only devices with the AP role write (requirement 7 / 8 / 9)───────────────
    // A required key that is missing or unusable blocks the plan: writing `encryption=sae-mixed`
    // without a usable key leaves the AP unable to authenticate anyone, which is worse than not
    // touching the wireless configuration at all.
    let key_problem = input
        .wifi
        .and_then(|resolved| crate::wifi_key::check(&resolved.profile.auth, input.wifi_key).err());
    if let Some(problem) = key_problem {
        let wifi = input.wifi.map(|resolved| &resolved.profile);
        plan.block(
            Message::new(problem.message_key())
                .param(
                    "ssid",
                    wifi.map(|profile| profile.ssid.clone()).unwrap_or_default(),
                )
                .param(
                    "reference",
                    wifi.map(|profile| profile.psk_ref.clone())
                        .unwrap_or_default(),
                ),
        );
    }

    if input.roles.ap {
        if let Some(resolved) = input.wifi.filter(|_| key_problem.is_none()) {
            let wifi = &resolved.profile;
            let radio = wifi.radio.clone();
            plan.notes.extend(resolved.notes.clone());
            if wifi.kvr.r || wifi.kvr.k || wifi.kvr.v {
                plan.notes.push(
                    Message::new("plan.kvr")
                        .flag("k", wifi.kvr.k)
                        .flag("v", wifi.kvr.v)
                        .flag("r", wifi.kvr.r)
                        .param("mobility_domain", wifi.kvr.mobility_domain.clone())
                        .flag("ft_over_ds", wifi.kvr.ft_over_ds),
                );
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
            if crate::wifi_key::requires_key(&wifi.auth) {
                // The op carries the **reference**; `apply` substitutes the real key immediately
                // before writing, so key material never reaches the dry-run text, the logs or
                // the UI.
                plan.ops.push(UciOp::set_secret(
                    "wireless",
                    &wifi_section,
                    "key",
                    &wifi.psk_ref,
                ));
                // Remove what earlier versions wrote: `key_ref` is not a valid OpenWrt key, so a
                // leftover would keep hostapd from starting.
                plan.ops
                    .push(UciOp::delete("wireless", &wifi_section, "key_ref"));
            } else {
                // An open network must carry no key at all.
                plan.ops
                    .push(UciOp::delete("wireless", &wifi_section, "key"));
                plan.ops
                    .push(UciOp::delete("wireless", &wifi_section, "key_ref"));
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
        plan.notes.push(Message::new("plan.wifi_not_applied"));
    }

    if let Some(profile) = input.profile {
        if !profile.is_empty() {
            plan.notes
                .push(Message::new("plan.profile_referenced").param("profile", profile.summary()));
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

    const TEST_KEY: &str = "correct horse battery";

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
            // A usable key, so tests exercise the normal (writable) path; the blocked path has its
            // own test.
            wifi_key: (wifi.is_some()).then_some(TEST_KEY),
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

    /// Core invariant: Gateway / Controller (with or without AP) never writes a bridge on its own,
    /// and a configuration without the AP role writes not even wireless.
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
                    "non-AP role combinations must be zero-write, roles={:?} plan={:?}",
                    roles,
                    plan
                );
            } else if !roles.enable_bridge() {
                // AP present but no bridge allowed (e.g. controller+ap): wireless writes are fine,
                // but bridge writes are not
                assert!(
                    !plan.reload_network,
                    "roles={:?} must not reload the network",
                    roles
                );
                assert!(
                    plan.bridges.is_empty(),
                    "roles={:?} must not have bridges",
                    roles
                );
                assert!(
                    !plan
                        .ops
                        .iter()
                        .any(|op| op.file == "network" && op.option.as_deref() == Some("ports")),
                    "roles={:?} must not write bridge ports",
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
        assert!(plan.dry_run_text().is_empty());
    }

    #[test]
    fn wifi_is_not_written_when_ap_absent_even_with_source() {
        let caps = caps();
        let wifi = resolved();
        let plan = build_write_plan(&input(Roles::controller_only(), &caps, Some(&wifi)));
        assert!(plan.is_empty());
        assert!(plan.wifi_radios.is_empty());
        assert!(plan.notes.iter().any(|n| n.key == "plan.wifi_not_applied"));
    }
}
