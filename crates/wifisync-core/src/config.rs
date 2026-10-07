//! Configuration model for `wifisync` itself (`/etc/config/wifisync`).
//!
//! The mapping to/from uci is **pure functions**: `from_sections` builds from parsed sections,
//! `to_uci_ops` produces write operations. This way all uci-related decisions can be fully
//! unit-tested on a host, and the system layer only has to run the commands.

use crate::admission::AdmissionRegistry;
use crate::backup::RestoreMode;
use crate::capability::Capabilities;
use crate::error::CoreResult;
use crate::failsafe::{FailsafeAction, FailsafeConfig};
use crate::plan::UciOp;
use crate::profile::KvrConfig;
use crate::profile::SyncMode;
use crate::role::Roles;
use crate::uci_file::UciSection;
use crate::wifi_source::{CustomWifi, WifiSourceConfig, WifiSourceKind};
use serde::{Deserialize, Serialize};

pub const UCI_FILE: &str = "wifisync";
pub const SECTION_MAIN: &str = "main";
pub const SECTION_SOURCE: &str = "source";
pub const SECTION_CUSTOM: &str = "custom";
pub const SECTION_FAILSAFE: &str = "failsafe";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifisyncConfig {
    pub device_id: String,
    /// Whether the user explicitly configured roles; if not, the first start derives defaults from
    /// the hardware (requirement 3: drop the AP role from the defaults on devices without
    /// wireless).
    pub roles_configured: bool,
    pub roles: Roles,
    /// Default bridge name; multiple bridges are structurally supported, and this is the default
    /// name.
    pub bridge_name: String,
    pub restore_mode: RestoreMode,
    pub sync_mode: SyncMode,
    /// LAN interfaces chosen by the user for the Gateway role (**recorded only, never modified**).
    pub gateway_lan_ifaces: Vec<String>,
    /// Connectivity probe target (the user explicitly confirms whether Controller and Gateway can
    /// reach each other).
    pub gateway_endpoint: Option<String>,
    /// Address of the Controller for the AP / Gateway link (`host[:port]`). Empty means "this
    /// device" when it takes the Controller role (reached over loopback, no credentials).
    pub controller_endpoint: Option<String>,
    /// Account used by this device to log in to a remote Controller (the password lives in the
    /// secret store, never in uci).
    pub controller_username: Option<String>,
    /// TCP port the Controller listens on.
    pub controller_port: u16,
    /// Address the Controller listens on.
    pub controller_bind: String,
    /// Whether the user has confirmed that "the local wireless configuration may be modified".
    pub local_wifi_change_confirmed: bool,
    pub wifi_source: WifiSourceConfig,
    pub failsafe: FailsafeConfig,
}

impl Default for WifisyncConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            roles_configured: false,
            roles: Roles {
                controller: true,
                ap: true,
                gateway: true,
            },
            bridge_name: crate::DEFAULT_BRIDGE.to_string(),
            restore_mode: RestoreMode::default(),
            sync_mode: SyncMode::default(),
            gateway_lan_ifaces: Vec::new(),
            gateway_endpoint: None,
            controller_endpoint: None,
            controller_username: None,
            controller_port: crate::link::DEFAULT_PORT,
            controller_bind: crate::link::DEFAULT_BIND.to_string(),
            local_wifi_change_confirmed: false,
            wifi_source: WifiSourceConfig::default(),
            failsafe: FailsafeConfig::default(),
        }
    }
}

fn section<'a>(sections: &'a [UciSection], name: &str) -> Option<&'a UciSection> {
    sections.iter().find(|s| s.name == name)
}

fn opt(sections: &[UciSection], section_name: &str, option: &str) -> Option<String> {
    section(sections, section_name).and_then(|s| s.option(option).map(|v| v.to_string()))
}

fn opt_bool(sections: &[UciSection], section_name: &str, option: &str, default: bool) -> bool {
    opt(sections, section_name, option)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn opt_u64(sections: &[UciSection], section_name: &str, option: &str, default: u64) -> u64 {
    opt(sections, section_name, option)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn opt_list(sections: &[UciSection], section_name: &str, option: &str) -> Vec<String> {
    section(sections, section_name)
        .map(|s| {
            let mut values: Vec<String> =
                s.list(option).into_iter().map(|v| v.to_string()).collect();
            if values.is_empty() {
                if let Some(single) = s.option(option) {
                    values = single.split_whitespace().map(|v| v.to_string()).collect();
                }
            }
            values
        })
        .unwrap_or_default()
}

impl WifisyncConfig {
    /// Lenient parsing: any missing item falls back to its default; startup is never refused
    /// because the configuration is incomplete.
    ///
    /// Assigning field by field instead of `..Default::default()` keeps the code readable section
    /// by section; the semantics are exactly equivalent to what clippy suggests.
    #[allow(clippy::field_reassign_with_default)]
    pub fn from_sections(sections: &[UciSection]) -> Self {
        let mut cfg = Self::default();

        cfg.device_id = opt(sections, SECTION_MAIN, "device_id").unwrap_or_default();
        cfg.roles_configured = opt_bool(sections, SECTION_MAIN, "roles_configured", false);
        if let Some(roles) = opt(sections, SECTION_MAIN, "roles") {
            if let Ok(parsed) = Roles::from_uci_value(&roles) {
                cfg.roles = parsed;
            }
        }
        if let Some(name) = opt(sections, SECTION_MAIN, "bridge_name") {
            if !name.trim().is_empty() {
                cfg.bridge_name = name;
            }
        }
        if let Some(mode) = opt(sections, SECTION_MAIN, "restore_mode") {
            cfg.restore_mode = match mode.as_str() {
                "full" => RestoreMode::Full,
                _ => RestoreMode::ManagedOnly,
            };
        }
        if let Some(mode) = opt(sections, SECTION_MAIN, "sync_mode") {
            if let Some(parsed) = SyncMode::from_str_opt(&mode) {
                cfg.sync_mode = parsed;
            }
        }
        cfg.gateway_lan_ifaces = opt_list(sections, SECTION_MAIN, "gateway_lan_ifaces");
        cfg.gateway_endpoint =
            opt(sections, SECTION_MAIN, "gateway_endpoint").filter(|v| !v.is_empty());
        cfg.controller_endpoint =
            opt(sections, SECTION_MAIN, "controller_endpoint").filter(|v| !v.is_empty());
        cfg.controller_username =
            opt(sections, SECTION_MAIN, "controller_username").filter(|v| !v.is_empty());
        if let Some(port) = opt(sections, SECTION_MAIN, "controller_port")
            .and_then(|v| v.parse::<u64>().ok())
            .and_then(|v| crate::link::check_port(v).ok())
        {
            cfg.controller_port = port;
        }
        if let Some(bind) = opt(sections, SECTION_MAIN, "controller_bind") {
            if crate::link::check_bind(&bind).is_ok() {
                cfg.controller_bind = bind;
            }
        }
        cfg.local_wifi_change_confirmed =
            opt_bool(sections, SECTION_MAIN, "local_wifi_change_confirmed", false);

        // Wi-Fi information source
        let mut source = WifiSourceConfig::default();
        if let Some(kind) = opt(sections, SECTION_SOURCE, "kind") {
            if let Some(parsed) = WifiSourceKind::from_str_opt(&kind) {
                source.kind = parsed;
            }
        }
        source.gateway_endpoint = opt(sections, SECTION_SOURCE, "gateway_endpoint")
            .filter(|v| !v.is_empty())
            .or_else(|| cfg.gateway_endpoint.clone());
        source.gateway_credential_ref =
            opt(sections, SECTION_SOURCE, "gateway_credential_ref").filter(|v| !v.is_empty());
        source.allow_fallback = opt_bool(sections, SECTION_SOURCE, "allow_fallback", false);

        if section(sections, SECTION_CUSTOM).is_some() {
            let mut kvr = KvrConfig::default();
            if let Some(domain) = opt(sections, SECTION_CUSTOM, "mobility_domain") {
                kvr.mobility_domain = domain;
            }
            kvr.k = opt_bool(sections, SECTION_CUSTOM, "ieee80211k", true);
            kvr.v = opt_bool(sections, SECTION_CUSTOM, "ieee80211v", true);
            kvr.r = opt_bool(sections, SECTION_CUSTOM, "ieee80211r", true);
            kvr.ft_over_ds = opt_bool(sections, SECTION_CUSTOM, "ft_over_ds", true);
            kvr.ft_psk_generate_local =
                opt_bool(sections, SECTION_CUSTOM, "ft_psk_generate_local", true);

            source.custom = Some(CustomWifi {
                // Empty means "not set": the resolver substitutes the generated default SSID.
                ssid: opt(sections, SECTION_CUSTOM, "ssid").unwrap_or_default(),
                auth: opt(sections, SECTION_CUSTOM, "auth").unwrap_or_else(|| "sae-mixed".into()),
                psk_ref: opt(sections, SECTION_CUSTOM, "psk_ref")
                    .unwrap_or_else(|| "custom".into()),
                band: opt(sections, SECTION_CUSTOM, "band").unwrap_or_else(|| "5g".into()),
                channel: opt(sections, SECTION_CUSTOM, "channel").and_then(|v| v.parse().ok()),
                hidden: opt_bool(sections, SECTION_CUSTOM, "hidden", false),
                kvr,
            });
        }
        cfg.wifi_source = source;

        // Failover
        let mut failsafe = FailsafeConfig::default();
        failsafe.enabled = opt_bool(sections, SECTION_FAILSAFE, "enabled", false);
        failsafe.apply_confirm_secs = opt_u64(sections, SECTION_FAILSAFE, "apply_confirm_secs", 90);
        failsafe.link_timeout_secs = opt_u64(sections, SECTION_FAILSAFE, "link_timeout_secs", 300);
        if let Some(action) = opt(sections, SECTION_FAILSAFE, "action") {
            failsafe.action = match action.as_str() {
                "reboot" => FailsafeAction::Reboot,
                _ => FailsafeAction::Revert,
            };
        }
        failsafe.keep_ssid = opt_bool(sections, SECTION_FAILSAFE, "keep_ssid", false);
        failsafe.heartbeat_endpoint = opt(sections, SECTION_FAILSAFE, "heartbeat_endpoint")
            .filter(|v| !v.is_empty())
            .or_else(|| {
                cfg.controller_endpoint.as_deref().map(|endpoint| {
                    crate::link::endpoint_with_port(endpoint, crate::link::DEFAULT_PORT)
                })
            })
            .or_else(|| cfg.gateway_endpoint.clone());
        cfg.failsafe = failsafe;

        cfg
    }

    /// Produce the operation sequence that writes the current configuration fully back to uci
    /// (idempotent).
    pub fn to_uci_ops(&self) -> Vec<UciOp> {
        let mut ops = Vec::new();
        let ensure = |ops: &mut Vec<UciOp>, name: &str, kind: &str| {
            ops.push(UciOp {
                kind: crate::plan::UciOpKind::Set,
                file: UCI_FILE.to_string(),
                section: name.to_string(),
                option: None,
                value: Some(kind.to_string()),
            });
        };
        ensure(&mut ops, SECTION_MAIN, "wifisync");
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "device_id",
            &self.device_id,
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "roles",
            self.roles.to_uci_value(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "roles_configured",
            if self.roles_configured { "1" } else { "0" },
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "bridge_name",
            &self.bridge_name,
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "restore_mode",
            match self.restore_mode {
                RestoreMode::ManagedOnly => "managed_only",
                RestoreMode::Full => "full",
            },
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "sync_mode",
            self.sync_mode.as_str(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "gateway_lan_ifaces",
            self.gateway_lan_ifaces.join(" "),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "gateway_endpoint",
            self.gateway_endpoint.clone().unwrap_or_default(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "controller_endpoint",
            self.controller_endpoint.clone().unwrap_or_default(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "controller_username",
            self.controller_username.clone().unwrap_or_default(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "controller_port",
            self.controller_port.to_string(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "controller_bind",
            &self.controller_bind,
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_MAIN,
            "local_wifi_change_confirmed",
            if self.local_wifi_change_confirmed {
                "1"
            } else {
                "0"
            },
        ));

        ensure(&mut ops, SECTION_SOURCE, "wifi_source");
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_SOURCE,
            "kind",
            self.wifi_source.kind.as_str(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_SOURCE,
            "gateway_endpoint",
            self.wifi_source
                .gateway_endpoint
                .clone()
                .unwrap_or_default(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_SOURCE,
            "gateway_credential_ref",
            self.wifi_source
                .gateway_credential_ref
                .clone()
                .unwrap_or_default(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_SOURCE,
            "allow_fallback",
            if self.wifi_source.allow_fallback {
                "1"
            } else {
                "0"
            },
        ));

        if let Some(custom) = &self.wifi_source.custom {
            ensure(&mut ops, SECTION_CUSTOM, "custom_wifi");
            ops.push(UciOp::set(UCI_FILE, SECTION_CUSTOM, "ssid", &custom.ssid));
            ops.push(UciOp::set(UCI_FILE, SECTION_CUSTOM, "auth", &custom.auth));
            ops.push(UciOp::set(
                UCI_FILE,
                SECTION_CUSTOM,
                "psk_ref",
                &custom.psk_ref,
            ));
            ops.push(UciOp::set(UCI_FILE, SECTION_CUSTOM, "band", &custom.band));
            ops.push(UciOp::set(
                UCI_FILE,
                SECTION_CUSTOM,
                "channel",
                custom.channel.map(|c| c.to_string()).unwrap_or_default(),
            ));
            ops.push(UciOp::set(
                UCI_FILE,
                SECTION_CUSTOM,
                "hidden",
                if custom.hidden { "1" } else { "0" },
            ));
            ops.push(UciOp::set(
                UCI_FILE,
                SECTION_CUSTOM,
                "mobility_domain",
                &custom.kvr.mobility_domain,
            ));
            ops.push(UciOp::set(
                UCI_FILE,
                SECTION_CUSTOM,
                "ieee80211k",
                if custom.kvr.k { "1" } else { "0" },
            ));
            ops.push(UciOp::set(
                UCI_FILE,
                SECTION_CUSTOM,
                "ieee80211v",
                if custom.kvr.v { "1" } else { "0" },
            ));
            ops.push(UciOp::set(
                UCI_FILE,
                SECTION_CUSTOM,
                "ieee80211r",
                if custom.kvr.r { "1" } else { "0" },
            ));
            ops.push(UciOp::set(
                UCI_FILE,
                SECTION_CUSTOM,
                "ft_over_ds",
                if custom.kvr.ft_over_ds { "1" } else { "0" },
            ));
            ops.push(UciOp::set(
                UCI_FILE,
                SECTION_CUSTOM,
                "ft_psk_generate_local",
                if custom.kvr.ft_psk_generate_local {
                    "1"
                } else {
                    "0"
                },
            ));
        }

        ensure(&mut ops, SECTION_FAILSAFE, "failsafe");
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_FAILSAFE,
            "enabled",
            if self.failsafe.enabled { "1" } else { "0" },
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_FAILSAFE,
            "apply_confirm_secs",
            self.failsafe.apply_confirm_secs.to_string(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_FAILSAFE,
            "link_timeout_secs",
            self.failsafe.link_timeout_secs.to_string(),
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_FAILSAFE,
            "action",
            match self.failsafe.action {
                FailsafeAction::Revert => "revert",
                FailsafeAction::Reboot => "reboot",
            },
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_FAILSAFE,
            "keep_ssid",
            if self.failsafe.keep_ssid { "1" } else { "0" },
        ));
        ops.push(UciOp::set(
            UCI_FILE,
            SECTION_FAILSAFE,
            "heartbeat_endpoint",
            self.failsafe.heartbeat_endpoint.clone().unwrap_or_default(),
        ));

        ops
    }

    /// Full validation before applying.
    pub fn validate(&self, caps: &Capabilities) -> CoreResult<()> {
        self.roles.validate(caps)?;
        if let Some(custom) = &self.wifi_source.custom {
            custom.kvr.validate().map_err(crate::CoreError::Invalid)?;
            // An empty SSID is allowed: the resolver substitutes the generated default SSID
            // (`Home_Wi-Fi_<6 random hex digits>`), which the user can still overwrite.
        }
        if self.wifi_source.kind == WifiSourceKind::Gateway
            && self
                .wifi_source
                .gateway_endpoint
                .as_deref()
                .unwrap_or("")
                .is_empty()
        {
            return Err(crate::CoreError::Invalid(
                "a Gateway address is required when the Gateway is used as Wi-Fi source".into(),
            ));
        }
        crate::link::check_bind(&self.controller_bind).map_err(crate::CoreError::Invalid)?;
        if self.controller_port == 0 {
            return Err(crate::CoreError::Invalid(
                "the Controller port must be between 1 and 65535".into(),
            ));
        }
        if let Some(name) = &self.controller_username {
            if !crate::link::valid_username(name) {
                return Err(crate::CoreError::Invalid(
                    "the Controller account name may only contain letters, digits and - _ . :"
                        .into(),
                ));
            }
        }
        if self.failsafe.enabled && self.failsafe.link_timeout_secs < 30 {
            return Err(crate::CoreError::Invalid(
                "failover timeout is too short (<30 s) and may trigger accidentally".into(),
            ));
        }
        Ok(())
    }

    /// Persisted view after appending runtime state (the admission registry).
    pub fn to_state_json(
        &self,
        admissions: &AdmissionRegistry,
        profile_version: u64,
    ) -> serde_json::Value {
        serde_json::json!({
            "device_id": self.device_id,
            "profile_version": profile_version,
            "admissions": admissions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uci_file;

    const SAMPLE: &str = r#"
config wifisync 'main'
	option device_id 'dev-abc'
	option roles 'controller ap gateway'
	option bridge_name 'br-lan'
	option restore_mode 'managed_only'
	option sync_mode 'auto'
	list gateway_lan_ifaces 'lan1'
	list gateway_lan_ifaces 'lan2'
	option gateway_endpoint '192.168.1.1'
	option local_wifi_change_confirmed '0'

config wifi_source 'source'
	option kind 'custom'
	option allow_fallback '0'

config custom_wifi 'custom'
	option ssid 'MyHome'
	option auth 'sae-mixed'
	option psk_ref 'custom'
	option band '5g'
	option channel '44'
	option mobility_domain '1a2b'
	option ieee80211k '1'
	option ieee80211v '1'
	option ieee80211r '1'

config failsafe 'failsafe'
	option enabled '1'
	option apply_confirm_secs '120'
	option link_timeout_secs '600'
	option action 'reboot'
	option keep_ssid '1'
"#;

    fn parse() -> Vec<UciSection> {
        uci_file::parse(SAMPLE)
    }

    fn caps_with_radio() -> Capabilities {
        Capabilities {
            radios: vec![crate::capability::RadioInfo {
                name: "radio0".into(),
                band: Some("5g".into()),
                channel: Some(36),
                supports_kvr: true,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn parses_full_config() {
        let cfg = WifisyncConfig::from_sections(&parse());
        assert_eq!(cfg.device_id, "dev-abc");
        assert!(cfg.roles.controller && cfg.roles.ap && cfg.roles.gateway);
        assert_eq!(cfg.gateway_lan_ifaces, vec!["lan1", "lan2"]);
        assert_eq!(cfg.wifi_source.kind, WifiSourceKind::Custom);
        let custom = cfg.wifi_source.custom.as_ref().unwrap();
        assert_eq!(custom.ssid, "MyHome");
        assert_eq!(custom.channel, Some(44));
        assert_eq!(custom.kvr.mobility_domain, "1a2b");
        assert!(cfg.failsafe.enabled);
        assert_eq!(cfg.failsafe.apply_confirm_secs, 120);
        assert_eq!(cfg.failsafe.action, FailsafeAction::Reboot);
        assert_eq!(
            cfg.failsafe.heartbeat_endpoint.as_deref(),
            Some("192.168.1.1")
        );
    }

    #[test]
    fn default_roles_follow_hardware_when_not_configured() {
        let cfg = WifisyncConfig::from_sections(&[]);
        assert!(!cfg.roles_configured);
        let no_wifi = Capabilities::default();
        assert!(
            !Roles::default_for(&no_wifi).ap,
            "the AP role must not be enabled by default without wireless"
        );
        let with_wifi = caps_with_radio();
        assert!(Roles::default_for(&with_wifi).ap);
    }

    #[test]
    fn missing_config_falls_back_to_defaults() {
        let cfg = WifisyncConfig::from_sections(&[]);
        assert_eq!(cfg.bridge_name, "br-lan");
        assert!(
            !cfg.failsafe.enabled,
            "failover must be disabled by default"
        );
        assert_eq!(cfg.restore_mode, RestoreMode::ManagedOnly);
        assert_eq!(cfg.wifi_source.kind, WifiSourceKind::ControllerSelf);
    }

    #[test]
    fn controller_link_defaults_and_overrides() {
        let cfg = WifisyncConfig::from_sections(&[]);
        assert_eq!(cfg.controller_port, 6550);
        assert_eq!(cfg.controller_bind, "0.0.0.0");
        assert!(cfg.controller_username.is_none());

        let text = "config wifisync 'main'\n\toption controller_port '7001'\n\toption controller_bind '::'\n\toption controller_username 'ap1'\n\toption controller_endpoint '10.0.0.2'\n";
        let cfg = WifisyncConfig::from_sections(&uci_file::parse(text));
        assert_eq!(cfg.controller_port, 7001);
        assert_eq!(cfg.controller_bind, "::");
        assert_eq!(cfg.controller_username.as_deref(), Some("ap1"));
        // The Controller address doubles as the failover heartbeat target, on the link port
        assert_eq!(
            cfg.failsafe.heartbeat_endpoint.as_deref(),
            Some("10.0.0.2:6550")
        );

        let ops = cfg.to_uci_ops();
        assert!(ops
            .iter()
            .any(|op| op.key() == "wifisync.main.controller_port"
                && op.value.as_deref() == Some("7001")));
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn invalid_link_settings_fall_back_or_fail_validation() {
        let text = "config wifisync 'main'\n\toption controller_port '99999'\n\toption controller_bind 'not-an-ip'\n";
        let cfg = WifisyncConfig::from_sections(&uci_file::parse(text));
        assert_eq!(cfg.controller_port, 6550);
        assert_eq!(cfg.controller_bind, "0.0.0.0");

        let mut cfg = WifisyncConfig::default();
        cfg.roles = Roles::controller_only();
        cfg.controller_bind = "lan".into();
        assert!(cfg.validate(&Capabilities::default()).is_err());
        cfg.controller_bind = "0.0.0.0".into();
        cfg.controller_username = Some("bad name".into());
        assert!(cfg.validate(&Capabilities::default()).is_err());
    }

    #[test]
    fn uci_ops_roundtrip_is_idempotent() {
        let cfg = WifisyncConfig::from_sections(&parse());
        let ops = cfg.to_uci_ops();
        assert!(ops.iter().any(|op| op.key() == "wifisync.main.roles"));
        assert!(ops
            .iter()
            .any(|op| op.key() == "wifisync.custom.mobility_domain"));
        // Each key is written only once
        let mut keys: Vec<String> = ops.iter().map(|op| op.key()).collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(before, keys.len(), "the same key must not be written twice");
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn gateway_source_requires_endpoint() {
        let mut cfg = WifisyncConfig::default();
        cfg.roles = Roles::controller_only();
        cfg.wifi_source.kind = WifiSourceKind::Gateway;
        cfg.wifi_source.gateway_endpoint = None;
        assert!(cfg.validate(&Capabilities::default()).is_err());
        cfg.wifi_source.gateway_endpoint = Some("192.168.1.1:8443".into());
        assert!(cfg.validate(&Capabilities::default()).is_ok());
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn failsafe_timeout_must_be_sane() {
        let mut cfg = WifisyncConfig::default();
        cfg.roles = Roles::controller_only();
        cfg.failsafe.enabled = true;
        cfg.failsafe.link_timeout_secs = 5;
        assert!(cfg.validate(&Capabilities::default()).is_err());
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn ap_role_validated_against_hardware() {
        let mut cfg = WifisyncConfig::default();
        cfg.roles = Roles::ap_only();
        assert!(cfg.validate(&Capabilities::default()).is_err());
    }
}
