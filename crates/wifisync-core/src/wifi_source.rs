//! Wi-Fi 信息源（需求 5 / 9）。
//!
//! Controller 的 Wi-Fi 信息有三个来源，且每个来源都有明确的启用/禁用条件：
//!
//! | 来源 | 取值 | 禁用条件 |
//! |------|------|---------|
//! | [`WifiSourceKind::ControllerSelf`] | Controller 本机无线配置 | Controller 上报「无 Wi-Fi」 |
//! | [`WifiSourceKind::Gateway`] | **只读**拉取 Gateway 的 Wi-Fi 档案 | Gateway 上报「无 Wi-Fi」/ 未配置 / 拉取失败 |
//! | [`WifiSourceKind::Custom`] | 用户在 LuCI 手工填写 | 始终可用 |
//!
//! 关键约束：**只有当本机承担 AP 角色时**，解析结果才会被写回本机无线配置；
//! 而 `custom` 来源在「AP 与 Controller 同设备」时必须由用户显式确认
//! （因为这会修改该设备自身的 Wi-Fi）。

use crate::capability::Capabilities;
use crate::error::{CoreError, CoreResult};
use crate::profile::{KvrConfig, WifiProfile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WifiSourceKind {
    /// 控制器自身的 Wi-Fi 信息
    ControllerSelf,
    /// 网关的 Wi-Fi 信息（只读）
    Gateway,
    /// 自定义信息
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

/// 自定义 Wi-Fi 参数（LuCI 表单）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomWifi {
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
            ssid: "WifiSync".to_string(),
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
    /// Gateway 探针/拉取端点，如 `192.168.1.1:8443`。
    pub gateway_endpoint: Option<String>,
    /// 拉取凭据引用名（真实密钥在 `/etc/wifisync/secrets/`）。
    pub gateway_credential_ref: Option<String>,
    pub custom: Option<CustomWifi>,
    /// 拉取失败时是否回退 `controller_self`（默认否，只告警）。
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

/// 解析所需的上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceContext {
    /// 本机角色是否包含 ap —— 决定结果是否写回本机无线配置。
    pub local_is_ap: bool,
    /// Controller 是否上报自身有 Wi-Fi。
    pub controller_reports_wifi: bool,
    /// Gateway 是否上报有 Wi-Fi（`None` = 未知/未联通）。
    pub gateway_reports_wifi: Option<bool>,
    /// 是否已成功从 Gateway 拉取到档案。
    pub gateway_profile_fetched: bool,
    /// 用户是否已显式确认「可以修改本机无线配置」。
    pub local_wifi_change_confirmed: bool,
    /// 可选：从 Gateway 拉取的档案（只读结果）。
    pub gateway_profile: Option<WifiProfile>,
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
        }
    }
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedWifi {
    pub profile: WifiProfile,
    /// 是否会把这份 Wi-Fi 配置写进本机（= 本机承担 AP 角色）。
    pub will_modify_local: bool,
    /// 是否需要用户二次确认才能应用（custom + 本机 AP）。
    pub requires_confirmation: bool,
    pub source: WifiSourceKind,
    pub notes: Vec<String>,
}

/// 某个来源在给定上下文下是否可用（供 LuCI 置灰 + daemon 双重校验）。
pub fn source_availability(
    config: &WifiSourceConfig,
    ctx: &SourceContext,
) -> Vec<(WifiSourceKind, bool, Option<String>)> {
    WifiSourceKind::ALL
        .iter()
        .map(|kind| {
            let reason = match kind {
                WifiSourceKind::ControllerSelf => {
                    if ctx.controller_reports_wifi {
                        None
                    } else {
                        Some("控制器自身没有 Wi-Fi".to_string())
                    }
                }
                WifiSourceKind::Gateway => {
                    let mut reason = None;
                    match ctx.gateway_reports_wifi {
                        Some(false) => reason = Some("网关上报没有 Wi-Fi".to_string()),
                        None => reason = Some("网关未选定或连通性未确认".to_string()),
                        Some(true) => {}
                    }
                    // 选中的来源才强制要求拉取成功
                    if config.kind == WifiSourceKind::Gateway
                        && reason.is_none()
                        && !ctx.gateway_profile_fetched
                        && !config.allow_fallback
                    {
                        reason = Some("尚未成功从网关拉取 Wi-Fi 信息".to_string());
                    }
                    reason
                }
                WifiSourceKind::Custom => None,
            };
            (*kind, reason.is_none(), reason)
        })
        .collect()
}

/// 解析出最终使用的 Wi-Fi 档案。
pub fn resolve(config: &WifiSourceConfig, ctx: &SourceContext) -> CoreResult<ResolvedWifi> {
    // 1. 来源可用性（与 UI 同一套判定，防绕过）
    let availability = source_availability(config, ctx);
    let (_, enabled, reason) = availability
        .iter()
        .find(|(kind, _, _)| *kind == config.kind)
        .cloned()
        .ok_or_else(|| CoreError::SourceUnavailable {
            source_name: config.kind.as_str().to_string(),
            reason: "未知来源".to_string(),
        })?;
    if !enabled {
        return Err(CoreError::SourceUnavailable {
            source_name: config.kind.as_str().to_string(),
            reason: reason.unwrap_or_else(|| "不可用".to_string()),
        });
    }

    // 2. 产出档案
    let mut notes = Vec::new();
    let profile = match config.kind {
        WifiSourceKind::ControllerSelf => {
            let mut p = WifiProfile::template("radio0");
            notes.push(
                "来源：控制器自身的 Wi-Fi 信息（仅作为模板下发，不改控制器本机配置）".to_string(),
            );
            p.kvr.mobility_domain = default_mobility_domain();
            p
        }
        WifiSourceKind::Gateway => match (&ctx.gateway_profile, ctx.gateway_profile_fetched) {
            (Some(p), true) => {
                notes.push("来源：网关的 Wi-Fi 信息（只读拉取，绝不写回网关）".to_string());
                p.clone()
            }
            _ => {
                return Err(CoreError::SourceUnavailable {
                    source_name: WifiSourceKind::Gateway.as_str().to_string(),
                    reason: "拉取网关 Wi-Fi 信息失败".to_string(),
                })
            }
        },
        WifiSourceKind::Custom => {
            let custom = config.custom.clone().ok_or_else(|| {
                CoreError::Invalid("选择自定义 Wi-Fi 信息但未提供参数".to_string())
            })?;
            let mut p = WifiProfile::template("radio0");
            p.ssid = custom.ssid;
            p.auth = custom.auth;
            p.psk_ref = custom.psk_ref;
            p.band = custom.band;
            p.channel = custom.channel;
            p.hidden = custom.hidden;
            p.kvr = custom.kvr;
            notes.push("来源：用户自定义 Wi-Fi 信息".to_string());
            p
        }
    };

    // 3. 是否写回本机：只有本机承担 AP 才会被修改
    let will_modify_local = ctx.local_is_ap;
    let requires_confirmation = will_modify_local && config.kind == WifiSourceKind::Custom;

    if requires_confirmation && !ctx.local_wifi_change_confirmed {
        // 需求 5：自定义 + AP 与 Controller 同设备 ⇒ 该设备 Wi-Fi 要被改，必须显式确认
        return Err(CoreError::LocalWifiChangeNotConfirmed);
    }
    if requires_confirmation {
        notes.push("注意：本设备同时承担 AP，应用自定义 Wi-Fi 信息会修改本机无线配置".to_string());
    }

    Ok(ResolvedWifi {
        profile,
        will_modify_local,
        requires_confirmation,
        source: config.kind,
        notes,
    })
}

/// 生成 4 位十六进制漫游域（无外部依赖的简单实现，仅用于默认值）。
pub fn default_mobility_domain() -> String {
    // 由调用方在真实环境下用随机数覆盖；这里给出稳定的合法值。
    "abcd".to_string()
}

/// 便捷工具：给定能力判断是否可以开启 KVR。
pub fn kvr_available(caps: &Capabilities) -> (bool, Option<String>) {
    if caps.radios.is_empty() {
        return (false, Some("本设备没有无线模块".to_string()));
    }
    if !caps.wpad_full {
        return (
            false,
            Some(
                "需要完整版 wpad（含 802.11k/v/r），请安装 `wpad` 而不是 `wpad-basic`".to_string(),
            ),
        );
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
        assert!(gateway.2.as_deref().unwrap().contains("没有 Wi-Fi"));
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
        // 但 UI 里 Custom 仍然可用
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
        assert!(reason.unwrap().contains("wpad"));
    }
}
