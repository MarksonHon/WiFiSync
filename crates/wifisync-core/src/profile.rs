//! `NetworkProfile`: the "network information" pushed by the Controller to the AP (requirement
//! 4 / 9 / 10).
//!
//! By default (`sync_mode = auto`) the AP follows the profile pushed by the Controller, including
//! the Wi-Fi information.

use crate::bridge::{BridgePlan, VlanDef};
use serde::{Deserialize, Serialize};

/// 802.11k/v/r parameters. **Not hard-coded locally**; produced by the Wi-Fi information source
/// and pushed along with the profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvrConfig {
    pub k: bool,
    pub v: bool,
    pub r: bool,
    /// Globally unique mobility domain (4 hex digits), allocated centrally by the Controller.
    /// Empty means "not configured": the resolver derives it from the SSID
    /// ([`crate::wifi_source::default_mobility_domain`]) so every AP ends up with the same value.
    pub mobility_domain: String,
    /// 802.11r over DS (more stable over relay / cross-device links).
    pub ft_over_ds: bool,
    pub ft_psk_generate_local: bool,
}

impl Default for KvrConfig {
    fn default() -> Self {
        Self {
            k: true,
            v: true,
            r: true,
            mobility_domain: String::new(),
            ft_over_ds: true,
            ft_psk_generate_local: true,
        }
    }
}

impl KvrConfig {
    /// Whether an explicitly configured mobility domain is well formed (4 hex digits).
    pub fn is_valid_mobility_domain(value: &str) -> bool {
        value.len() == 4 && value.chars().all(|c| c.is_ascii_hexdigit())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.mobility_domain.is_empty() {
            // Not configured: the resolver derives it from the SSID (see `wifi_source`).
            return Ok(());
        }
        if !Self::is_valid_mobility_domain(&self.mobility_domain) {
            return Err(format!(
                "mobility_domain must be 4 hex digits, or empty to derive it from the SSID, got `{}`",
                self.mobility_domain
            ));
        }
        Ok(())
    }
}

/// Wi-Fi profile of a single radio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiProfile {
    /// Radio name in uci, e.g. `radio0`.
    pub radio: String,
    pub ssid: String,
    /// `none` / `psk2` / `sae` / `sae-mixed` ...
    pub auth: String,
    /// Key reference (the real key is stored under `/etc/wifisync/secrets/`; the profile only
    /// holds the reference name).
    pub psk_ref: String,
    /// `2g` / `5g` / `6g`
    pub band: String,
    pub channel: Option<u32>,
    pub width_mhz: Option<u32>,
    pub hidden: bool,
    pub disabled: bool,
    pub kvr: KvrConfig,
}

impl WifiProfile {
    /// A radio template. The SSID is intentionally empty: the resolver always fills it in with the
    /// real or generated name, and an empty one is rejected by validation instead of silently
    /// overriding the network with a placeholder.
    pub fn template(radio: impl Into<String>) -> Self {
        Self {
            radio: radio.into(),
            ssid: String::new(),
            auth: "sae-mixed".to_string(),
            psk_ref: "default".to_string(),
            band: "5g".to_string(),
            channel: None,
            width_mhz: None,
            hidden: false,
            disabled: false,
            kvr: KvrConfig::default(),
        }
    }
}

/// Complete network profile: bridges / VLANs / Wi-Fi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkProfile {
    /// Monotonically increasing; on conflict the higher version wins.
    pub version: u64,
    pub updated_at: crate::Timestamp,
    /// List structure, supporting multiple bridges.
    pub bridges: Vec<BridgePlan>,
    /// Reserved: VLAN cross-device bridge sync.
    pub vlans: Vec<VlanDef>,
    pub wifi: Vec<WifiProfile>,
}

impl Default for NetworkProfile {
    fn default() -> Self {
        Self {
            version: 1,
            updated_at: 0,
            bridges: Vec::new(),
            vlans: Vec::new(),
            wifi: Vec::new(),
        }
    }
}

impl NetworkProfile {
    pub fn is_empty(&self) -> bool {
        self.bridges.is_empty() && self.vlans.is_empty() && self.wifi.is_empty()
    }

    /// Whether to accept the other side's profile (only a higher version is accepted).
    pub fn accepts(&self, incoming: &Self) -> bool {
        incoming.version > self.version
    }

    /// Merge: the higher version wins; equal versions keep the local copy (avoids flapping).
    pub fn merge(&self, incoming: &Self) -> Self {
        if self.accepts(incoming) {
            incoming.clone()
        } else {
            self.clone()
        }
    }

    /// Validate the internal consistency of the profile.
    pub fn validate(&self) -> Result<(), String> {
        for wifi in &self.wifi {
            if wifi.ssid.is_empty() {
                return Err(format!("empty SSID for {}", wifi.radio));
            }
            if wifi.auth != "none" && wifi.psk_ref.is_empty() {
                return Err(format!(
                    "{} uses encryption but has no key reference",
                    wifi.radio
                ));
            }
            wifi.kvr.validate()?;
        }
        for bridge in &self.bridges {
            if bridge.name.is_empty() {
                return Err("bridge name must not be empty".to_string());
            }
        }
        Ok(())
    }

    /// Language neutral one-line summary (used inside translated messages).
    pub fn summary(&self) -> String {
        format!(
            "v{}: {} bridge / {} vlan / {} wifi",
            self.version,
            self.bridges.len(),
            self.vlans.len(),
            self.wifi.len()
        )
    }
}

/// AP sync mode (requirement 4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Default: follow what the Controller pushes.
    #[default]
    Auto,
    /// Local temporary override; Controller version updates do not overwrite it, and the UI must
    /// warn prominently.
    LocalOverride,
}

impl SyncMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SyncMode::Auto => "auto",
            SyncMode::LocalOverride => "local_override",
        }
    }

    pub fn from_str_opt(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(SyncMode::Auto),
            "local_override" => Some(SyncMode::LocalOverride),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(version: u64, ssid: &str) -> NetworkProfile {
        NetworkProfile {
            version,
            updated_at: version as i64,
            wifi: vec![WifiProfile {
                ssid: ssid.to_string(),
                ..WifiProfile::template("radio0")
            }],
            ..Default::default()
        }
    }

    #[test]
    fn higher_version_wins() {
        let local = profile(1, "home");
        let incoming = profile(2, "home-2");
        assert!(local.accepts(&incoming));
        assert_eq!(local.merge(&incoming).wifi[0].ssid, "home-2");
        assert_eq!(incoming.merge(&local).wifi[0].ssid, "home-2");
    }

    #[test]
    fn same_version_keeps_local() {
        let local = profile(7, "local");
        let incoming = profile(7, "remote");
        assert_eq!(local.merge(&incoming).wifi[0].ssid, "local");
    }

    #[test]
    fn validation_catches_bad_mobility_domain() {
        let mut p = profile(1, "home");
        p.wifi[0].kvr.mobility_domain = "zzzz".into();
        assert!(p.validate().is_err());
        p.wifi[0].kvr.mobility_domain = "1a2b".into();
        assert!(p.validate().is_ok());
    }

    #[test]
    fn an_unset_mobility_domain_is_valid_because_it_is_derived() {
        let p = profile(1, "home");
        assert!(p.wifi[0].kvr.mobility_domain.is_empty());
        assert!(p.validate().is_ok());
    }

    #[test]
    fn validation_catches_empty_ssid_and_missing_key() {
        let p = profile(1, "");
        assert!(p.validate().is_err());
        let mut p = profile(1, "home");
        p.wifi[0].psk_ref = String::new();
        assert!(p.validate().is_err());
        p.wifi[0].auth = "none".into();
        assert!(p.validate().is_ok());
    }

    #[test]
    fn sync_mode_roundtrip() {
        assert_eq!(SyncMode::default(), SyncMode::Auto);
        assert_eq!(
            SyncMode::from_str_opt("local_override"),
            Some(SyncMode::LocalOverride)
        );
        assert_eq!(SyncMode::from_str_opt("nope"), None);
    }
}
