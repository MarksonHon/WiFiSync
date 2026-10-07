//! Wi-Fi information sources (requirement 5 / 9).
//!
//! The Controller's Wi-Fi information has three sources, each with explicit enable/disable
//! conditions:
//!
//! | Source | Value | Disable condition |
//! |------|------|---------|
//! | [`WifiSourceKind::ControllerSelf`] | the Controller's own wireless config | the Controller reports "no Wi-Fi" |
//! | [`WifiSourceKind::Gateway`] | **read-only** fetch of the Gateway's Wi-Fi profile | the Gateway reports "no Wi-Fi" / not configured / fetch failed |
//! | [`WifiSourceKind::Custom`] | entered by hand by the user in LuCI | always available |
//!
//! Key constraint: the resolved result is written back to the local wireless configuration **only
//! when this device takes the AP role**; and the `custom` source must be explicitly confirmed by
//! the user when "AP and Controller are the same device" (because that modifies the device's own
//! Wi-Fi).

use crate::capability::Capabilities;
use crate::error::{CoreError, CoreResult};
use crate::message::Message;
use crate::profile::{KvrConfig, WifiProfile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WifiSourceKind {
    /// The controller's own Wi-Fi information
    ControllerSelf,
    /// The gateway's Wi-Fi information (read-only)
    Gateway,
    /// Custom information
    Custom,
}

impl WifiSourceKind {
    pub const ALL: [Self; 3] = [Self::ControllerSelf, Self::Gateway, Self::Custom];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ControllerSelf => "controller_self",
            Self::Gateway => "gateway",
            Self::Custom => "custom",
        }
    }

    pub fn from_str_opt(value: &str) -> Option<Self> {
        match value {
            "controller_self" => Some(Self::ControllerSelf),
            "gateway" => Some(Self::Gateway),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// Custom Wi-Fi parameters (LuCI form).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomWifi {
    /// Empty means "not configured": the resolver substitutes the generated default SSID
    /// (`Home_Wi-Fi_<6 random hex digits>`), which the user can still overwrite.
    pub ssid: String,
    pub auth: String,
    pub psk_ref: String,
    pub band: String,
    pub channel: Option<u32>,
    pub hidden: bool,
    pub kvr: KvrConfig,
}

impl Default for CustomWifi {
    fn default() -> Self {
        Self {
            ssid: String::new(),
            auth: "sae-mixed".to_string(),
            psk_ref: "custom".to_string(),
            band: "5g".to_string(),
            channel: None,
            hidden: false,
            kvr: KvrConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiSourceConfig {
    pub kind: WifiSourceKind,
    /// Gateway probe/fetch endpoint, e.g. `192.168.1.1:8443`.
    pub gateway_endpoint: Option<String>,
    /// Fetch credential reference name (the real key is in `/etc/wifisync/secrets/`).
    pub gateway_credential_ref: Option<String>,
    pub custom: Option<CustomWifi>,
    /// Whether to fall back to `controller_self` when the fetch fails (default no; warn only).
    pub allow_fallback: bool,
}

impl Default for WifiSourceConfig {
    fn default() -> Self {
        Self {
            kind: WifiSourceKind::ControllerSelf,
            gateway_endpoint: None,
            gateway_credential_ref: None,
            custom: None,
            allow_fallback: false,
        }
    }
}

/// Context required for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceContext {
    /// Whether the local roles include ap — decides whether the result is written back to the
    /// local wireless configuration.
    pub local_is_ap: bool,
    /// Whether the Controller reports that it has Wi-Fi.
    pub controller_reports_wifi: bool,
    /// Whether the Gateway reports Wi-Fi (`None` = unknown / not reachable).
    pub gateway_reports_wifi: Option<bool>,
    /// Whether a profile was successfully fetched from the Gateway.
    pub gateway_profile_fetched: bool,
    /// Whether the user has explicitly confirmed that "the local wireless configuration may be
    /// modified".
    pub local_wifi_change_confirmed: bool,
    /// Optional: the profile fetched from the Gateway (read-only result).
    pub gateway_profile: Option<WifiProfile>,
    /// Optional: **this device's own** Wi-Fi profile, read from its `/etc/config/wireless`
    /// (the real SSID/encryption/band/channel rather than a placeholder). `None` when this device
    /// has no configured AP interface.
    pub controller_profile: Option<WifiProfile>,
    /// SSID to fall back to when neither the device nor the user provides one
    /// (`Home_Wi-Fi_<6 random hex digits>`). Generated once and persisted by the caller, because
    /// the core layer has neither randomness nor I/O.
    pub default_ssid: Option<String>,
}

impl Default for SourceContext {
    fn default() -> Self {
        Self {
            local_is_ap: false,
            controller_reports_wifi: true,
            gateway_reports_wifi: None,
            gateway_profile_fetched: false,
            local_wifi_change_confirmed: false,
            gateway_profile: None,
            controller_profile: None,
            default_ssid: None,
        }
    }
}

/// Resolution result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedWifi {
    pub profile: WifiProfile,
    /// Whether this Wi-Fi configuration will be written locally (= this device takes the AP role).
    pub will_modify_local: bool,
    /// Whether a second user confirmation is needed before applying (custom + local AP).
    pub requires_confirmation: bool,
    pub source: WifiSourceKind,
    /// Explanations for the UI (translated by the front end).
    pub notes: Vec<Message>,
}

/// Whether a source is available under the given context (used for LuCI greying out plus a
/// second check in the daemon).
pub fn source_availability(
    config: &WifiSourceConfig,
    ctx: &SourceContext,
) -> Vec<(WifiSourceKind, bool, Option<Message>)> {
    WifiSourceKind::ALL
        .iter()
        .map(|kind| {
            let reason = match kind {
                WifiSourceKind::ControllerSelf => {
                    if ctx.controller_reports_wifi {
                        None
                    } else {
                        Some(Message::new("wifi_source.reason.controller_no_wifi"))
                    }
                }
                WifiSourceKind::Gateway => {
                    let mut reason = None;
                    match ctx.gateway_reports_wifi {
                        Some(false) => {
                            reason = Some(Message::new("wifi_source.reason.gateway_no_wifi"))
                        }
                        None => {
                            reason = Some(Message::new("wifi_source.reason.gateway_not_selected"))
                        }
                        Some(true) => {}
                    }
                    // Only the selected source requires a successful fetch
                    if config.kind == WifiSourceKind::Gateway
                        && reason.is_none()
                        && !ctx.gateway_profile_fetched
                        && !config.allow_fallback
                    {
                        reason = Some(Message::new("wifi_source.reason.gateway_fetch_pending"));
                    }
                    reason
                }
                WifiSourceKind::Custom => None,
            };
            (*kind, reason.is_none(), reason)
        })
        .collect()
}

/// Resolve the Wi-Fi profile to actually use.
pub fn resolve(config: &WifiSourceConfig, ctx: &SourceContext) -> CoreResult<ResolvedWifi> {
    // 1. Source availability (the same decision the UI uses, to prevent bypassing)
    let availability = source_availability(config, ctx);
    let (_, enabled, _) = availability
        .iter()
        .find(|(kind, _, _)| *kind == config.kind)
        .cloned()
        .ok_or_else(|| CoreError::SourceUnavailable {
            source_name: config.kind.as_str().to_string(),
            reason: Message::new("wifi_source.reason.unknown").to_string(),
        })?;
    if !enabled {
        return Err(CoreError::SourceUnavailable {
            source_name: config.kind.as_str().to_string(),
            reason: Message::new("wifi_source.reason.unavailable").to_string(),
        });
    }

    // 2. Build the profile
    let mut notes = Vec::new();
    let mut profile = match config.kind {
        WifiSourceKind::ControllerSelf => {
            notes.push(Message::new("wifi_source.note.controller_self"));
            // The device's own Wi-Fi configuration when it has one — never a placeholder: this is
            // what used to push an invented SSID onto the network. Without a configured AP
            // interface the template is used and the generated default SSID fills in the name.
            ctx.controller_profile
                .clone()
                .unwrap_or_else(|| WifiProfile::template("radio0"))
        }
        WifiSourceKind::Gateway => match (&ctx.gateway_profile, ctx.gateway_profile_fetched) {
            (Some(p), true) => {
                notes.push(Message::new("wifi_source.note.gateway"));
                p.clone()
            }
            _ => {
                return Err(CoreError::SourceUnavailable {
                    source_name: WifiSourceKind::Gateway.as_str().to_string(),
                    reason: Message::new("wifi_source.reason.gateway_fetch_failed").to_string(),
                })
            }
        },
        WifiSourceKind::Custom => {
            let custom = config.custom.clone().ok_or_else(|| {
                CoreError::Invalid("custom Wi-Fi source selected without parameters".to_string())
            })?;
            let mut p = WifiProfile::template("radio0");
            p.ssid = custom.ssid;
            p.auth = custom.auth;
            p.psk_ref = custom.psk_ref;
            p.band = custom.band;
            p.channel = custom.channel;
            p.hidden = custom.hidden;
            p.kvr = custom.kvr;
            notes.push(Message::new("wifi_source.note.custom"));
            p
        }
    };

    // 2b. Fill in what is derived rather than configured: the SSID when nobody supplied one, and
    // the mobility domain when there is no explicit value. Deriving the domain from the SSID means
    // renaming the network changes the domain on every AP at once — which is what the versioned
    // profile distribution is for.
    if profile.ssid.trim().is_empty() {
        profile.ssid = default_ssid(ctx);
    }
    if profile.kvr.mobility_domain.is_empty() {
        profile.kvr.mobility_domain = default_mobility_domain(&profile.ssid);
    }

    // 3. Whether to write back locally: only a device that takes the AP role is modified
    let will_modify_local = ctx.local_is_ap;
    let requires_confirmation = will_modify_local && config.kind == WifiSourceKind::Custom;

    if requires_confirmation && !ctx.local_wifi_change_confirmed {
        // Requirement 5: custom + AP and Controller on the same device ⇒ that device's Wi-Fi is
        // modified, so explicit confirmation is required
        return Err(CoreError::LocalWifiChangeNotConfirmed);
    }
    if requires_confirmation {
        notes.push(Message::new("wifi_source.note.local_wifi_modified"));
    }

    Ok(ResolvedWifi {
        profile,
        will_modify_local,
        requires_confirmation,
        source: config.kind,
        notes,
    })
}

/// Prefix of the SSID generated when neither the device nor the user provides one.
pub const DEFAULT_SSID_PREFIX: &str = "Home_Wi-Fi_";

/// SSID used when nothing is configured: the value generated by the caller when it has one,
/// otherwise a deterministic placeholder (the core layer has no randomness and no I/O).
fn default_ssid(ctx: &SourceContext) -> String {
    ctx.default_ssid
        .clone()
        .filter(|ssid| !ssid.trim().is_empty())
        .unwrap_or_else(|| format!("{}000000", DEFAULT_SSID_PREFIX))
}

/// Derive a 4-hex-digit mobility domain from the SSID.
///
/// Derivation rather than allocation: every device that knows the same SSID computes the same
/// value with no coordination at all, and renaming the network changes the domain everywhere at
/// once. An explicitly configured domain always wins over this.
///
/// FNV-1a instead of a cryptographic digest on purpose: the value is public (it is broadcast in
/// beacons) and only 16 bits wide, so a crypto dependency in `wifisync-core` would buy nothing.
pub fn default_mobility_domain(ssid: &str) -> String {
    const FNV_OFFSET_BASIS: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in ssid.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{:04x}", hash & 0xffff)
}

/// Convenience helper: decide whether KVR can be enabled given the capabilities.
pub fn kvr_available(caps: &Capabilities) -> (bool, Option<Message>) {
    if caps.radios.is_empty() {
        return (false, Some(Message::new("wifi_source.kvr.no_radio")));
    }
    if !caps.wpad_full {
        return (false, Some(Message::new("wifi_source.kvr.wpad_basic")));
    }
    (true, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_controller_only() -> SourceContext {
        SourceContext {
            local_is_ap: false,
            ..Default::default()
        }
    }

    #[test]
    fn custom_on_controller_only_device_does_not_touch_local_wifi() {
        let config = WifiSourceConfig {
            kind: WifiSourceKind::Custom,
            custom: Some(CustomWifi::default()),
            ..Default::default()
        };
        let resolved = resolve(&config, &ctx_controller_only()).unwrap();
        assert!(!resolved.will_modify_local);
        assert!(!resolved.requires_confirmation);
    }

    #[test]
    fn custom_on_shared_device_requires_confirmation() {
        let config = WifiSourceConfig {
            kind: WifiSourceKind::Custom,
            custom: Some(CustomWifi::default()),
            ..Default::default()
        };
        let ctx = SourceContext {
            local_is_ap: true,
            ..Default::default()
        };
        assert_eq!(
            resolve(&config, &ctx),
            Err(CoreError::LocalWifiChangeNotConfirmed)
        );

        let confirmed = SourceContext {
            local_wifi_change_confirmed: true,
            ..ctx
        };
        let resolved = resolve(&config, &confirmed).unwrap();
        assert!(resolved.will_modify_local);
        assert!(resolved.requires_confirmation);
    }

    #[test]
    fn gateway_source_disabled_when_gateway_reports_no_wifi() {
        let config = WifiSourceConfig {
            kind: WifiSourceKind::Gateway,
            gateway_endpoint: Some("192.168.1.1:8443".into()),
            ..Default::default()
        };
        let ctx = SourceContext {
            local_is_ap: true,
            gateway_reports_wifi: Some(false),
            ..Default::default()
        };
        let availability = source_availability(&config, &ctx);
        let gateway = availability
            .iter()
            .find(|(k, _, _)| *k == WifiSourceKind::Gateway)
            .unwrap();
        assert!(!gateway.1);
        assert_eq!(
            gateway.2.as_ref().unwrap().key,
            "wifi_source.reason.gateway_no_wifi"
        );
        assert!(matches!(
            resolve(&config, &ctx),
            Err(CoreError::SourceUnavailable { .. })
        ));
    }

    #[test]
    fn controller_self_disabled_when_controller_reports_no_wifi() {
        let config = WifiSourceConfig {
            kind: WifiSourceKind::ControllerSelf,
            ..Default::default()
        };
        let ctx = SourceContext {
            controller_reports_wifi: false,
            ..Default::default()
        };
        assert!(matches!(
            resolve(&config, &ctx),
            Err(CoreError::SourceUnavailable { .. })
        ));
        // but Custom is still available in the UI
        let availability = source_availability(&config, &ctx);
        let custom = availability
            .iter()
            .find(|(k, _, _)| *k == WifiSourceKind::Custom)
            .unwrap();
        assert!(custom.1);
    }

    #[test]
    fn gateway_source_works_when_profile_fetched() {
        let config = WifiSourceConfig {
            kind: WifiSourceKind::Gateway,
            gateway_endpoint: Some("192.168.1.1:8443".into()),
            ..Default::default()
        };
        let ctx = SourceContext {
            local_is_ap: true,
            gateway_reports_wifi: Some(true),
            gateway_profile_fetched: true,
            gateway_profile: Some(WifiProfile {
                ssid: "from-gateway".into(),
                ..WifiProfile::template("radio0")
            }),
            ..Default::default()
        };
        let resolved = resolve(&config, &ctx).unwrap();
        assert_eq!(resolved.profile.ssid, "from-gateway");
        assert!(resolved.will_modify_local);
    }

    #[test]
    fn kvr_availability_explains_wpad() {
        let caps = Capabilities {
            radios: vec![crate::capability::RadioInfo {
                name: "radio0".into(),
                band: None,
                channel: None,
                supports_kvr: true,
            }],
            wpad_full: false,
            ..Default::default()
        };
        let (ok, reason) = kvr_available(&caps);
        assert!(!ok);
        assert!(reason.unwrap().key.contains("wpad"));
    }

    #[test]
    fn the_mobility_domain_is_derived_deterministically_from_the_ssid() {
        let domain = default_mobility_domain("Home_Wi-Fi_a1b2c3");

        assert_eq!(domain, default_mobility_domain("Home_Wi-Fi_a1b2c3"));
        assert_eq!(domain.len(), 4);
        assert!(domain.chars().all(|c| c.is_ascii_hexdigit()));
        // Two different networks must not end up sharing one domain.
        assert_ne!(domain, default_mobility_domain("Home_Wi-Fi_a1b2c4"));
    }

    #[test]
    fn controller_self_uses_the_devices_own_wifi_instead_of_a_placeholder() {
        let config = WifiSourceConfig::default(); // kind = controller_self
        let ctx = SourceContext {
            local_is_ap: true,
            controller_profile: Some(WifiProfile {
                ssid: "MyHome".into(),
                auth: "psk2".into(),
                ..WifiProfile::template("radio1")
            }),
            ..Default::default()
        };

        let resolved = resolve(&config, &ctx).unwrap();

        assert_eq!(resolved.profile.ssid, "MyHome");
        assert_eq!(resolved.profile.auth, "psk2");
        assert_eq!(resolved.profile.radio, "radio1");
        // Nobody configured a domain, so it follows the SSID.
        assert_eq!(
            resolved.profile.kvr.mobility_domain,
            default_mobility_domain("MyHome")
        );
    }

    #[test]
    fn a_device_without_its_own_wifi_uses_the_generated_name() {
        let config = WifiSourceConfig::default();
        let ctx = SourceContext {
            default_ssid: Some("Home_Wi-Fi_4f2a91".into()),
            ..Default::default()
        };

        let resolved = resolve(&config, &ctx).unwrap();

        assert_eq!(resolved.profile.ssid, "Home_Wi-Fi_4f2a91");
        assert_eq!(
            resolved.profile.kvr.mobility_domain,
            default_mobility_domain("Home_Wi-Fi_4f2a91")
        );
    }

    #[test]
    fn a_custom_source_without_an_ssid_uses_the_generated_name() {
        let config = WifiSourceConfig {
            kind: WifiSourceKind::Custom,
            custom: Some(CustomWifi::default()),
            ..Default::default()
        };
        let ctx = SourceContext {
            default_ssid: Some("Home_Wi-Fi_abcdef".into()),
            ..Default::default()
        };

        let resolved = resolve(&config, &ctx).unwrap();

        assert_eq!(resolved.profile.ssid, "Home_Wi-Fi_abcdef");
    }

    #[test]
    fn an_explicit_mobility_domain_wins_over_the_derived_one() {
        let config = WifiSourceConfig {
            kind: WifiSourceKind::Custom,
            custom: Some(CustomWifi {
                ssid: "Whatever".into(),
                kvr: KvrConfig {
                    mobility_domain: "1a2b".into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let ctx = SourceContext {
            local_is_ap: true,
            local_wifi_change_confirmed: true,
            default_ssid: Some("Home_Wi-Fi_abcdef".into()),
            ..Default::default()
        };

        let resolved = resolve(&config, &ctx).unwrap();

        assert_eq!(resolved.profile.kvr.mobility_domain, "1a2b");
    }
}
