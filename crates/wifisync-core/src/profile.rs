//! `NetworkProfile`：Controller 下发给 AP 的「网络信息」（需求 4 / 9 / 10）。
//!
//! AP 默认（`sync_mode = auto`）跟随 Controller 下发的档案，包含 Wi-Fi 信息。

use crate::bridge::{BridgePlan, VlanDef};
use serde::{Deserialize, Serialize};

/// 802.11k/v/r 参数。**不在本地硬编码**，由 Wi-Fi 信息源产出后随档案下发。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvrConfig {
    pub k: bool,
    pub v: bool,
    pub r: bool,
    /// 全局唯一的漫游域（4 位十六进制），由 Controller 统一分配。
    pub mobility_domain: String,
    /// 802.11r 走 DS（中继 / 跨设备链路下更稳）。
    pub ft_over_ds: bool,
    pub ft_psk_generate_local: bool,
}

impl Default for KvrConfig {
    fn default() -> Self {
        Self {
            k: true,
            v: true,
            r: true,
            mobility_domain: "abcd".to_string(),
            ft_over_ds: true,
            ft_psk_generate_local: true,
        }
    }
}

impl KvrConfig {
    /// 缺省且合法的漫游域。
    pub fn is_valid_mobility_domain(value: &str) -> bool {
        value.len() == 4 && value.chars().all(|c| c.is_ascii_hexdigit())
    }

    pub fn validate(&self) -> Result<(), String> {
        if !Self::is_valid_mobility_domain(&self.mobility_domain) {
            return Err(format!(
                "mobility_domain 必须是 4 位十六进制，当前为 `{}`",
                self.mobility_domain
            ));
        }
        Ok(())
    }
}

/// 单个 radio 的 Wi-Fi 档案。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiProfile {
    /// uci radio 名，如 `radio0`。
    pub radio: String,
    pub ssid: String,
    /// `none` / `psk2` / `sae` / `sae-mixed` ...
    pub auth: String,
    /// 密钥引用（真实密钥保存在 `/etc/wifisync/secrets/` 下，档案里只放引用名）。
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
    pub fn template(radio: impl Into<String>) -> Self {
        Self {
            radio: radio.into(),
            ssid: "OpenWrt".to_string(),
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

/// 完整网络档案：网桥 / VLAN / Wi-Fi。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkProfile {
    /// 单调递增；冲突时高版本胜。
    pub version: u64,
    pub updated_at: crate::Timestamp,
    /// 列表结构，支持多网桥。
    pub bridges: Vec<BridgePlan>,
    /// 预留：VLAN 跨设备网桥同步。
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

    /// 是否接受对方档案（版本更高才接受）。
    pub fn accepts(&self, incoming: &Self) -> bool {
        incoming.version > self.version
    }

    /// 合并：高版本赢；相同版本保留本地（避免抖动）。
    pub fn merge(&self, incoming: &Self) -> Self {
        if self.accepts(incoming) {
            incoming.clone()
        } else {
            self.clone()
        }
    }

    /// 校验档案内部一致性。
    pub fn validate(&self) -> Result<(), String> {
        for wifi in &self.wifi {
            if wifi.ssid.is_empty() {
                return Err(format!("{} 的 SSID 为空", wifi.radio));
            }
            if wifi.auth != "none" && wifi.psk_ref.is_empty() {
                return Err(format!("{} 使用了加密但缺少密钥引用", wifi.radio));
            }
            wifi.kvr.validate()?;
        }
        for bridge in &self.bridges {
            if bridge.name.is_empty() {
                return Err("网桥名为空".to_string());
            }
        }
        Ok(())
    }

    pub fn summary(&self) -> String {
        format!(
            "v{}: {} 网桥 / {} VLAN / {} Wi-Fi",
            self.version,
            self.bridges.len(),
            self.vlans.len(),
            self.wifi.len()
        )
    }
}

/// AP 的同步模式（需求 4）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// 默认：跟随 Controller 下发。
    #[default]
    Auto,
    /// 本地临时覆盖，Controller 版本更新不覆盖本地，UI 需显著警示。
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
