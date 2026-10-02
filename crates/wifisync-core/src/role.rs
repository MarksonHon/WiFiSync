//! 角色模型（需求 3 / 5 / 7 / 8）。
//!
//! 三种角色是**能力集合**，一台设备可以同时承担多个：
//!
//! | 角色 | 默认 | 对本机网络的改动 |
//! |------|------|-----------------|
//! | `gateway` | 有无线时默认开 | **无**（只让用户选 LAN 接口用于识别/探测） |
//! | `controller` | 有无线时默认开 | **无**（只做新 AP 准入 + 网络信息下发） |
//! | `ap` | 有无线时默认开；**无无线强制关闭** | 应用下发的 Wi-Fi / 网桥信息 |
//!
//! 只有「纯 AP」（有 ap 且没有 gateway 也没有 controller）才自动把所有网口组成 `br-lan`。

use crate::capability::Capabilities;
use crate::error::{CoreError, CoreResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Roles {
    pub controller: bool,
    pub ap: bool,
    pub gateway: bool,
}

/// 角色被自动调整的原因，供 UI 展示（需求 3：无无线时取消默认 AP 并禁用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleAdjustment {
    ApRemovedNoWifi,
}

impl RoleAdjustment {
    pub fn message(&self) -> &'static str {
        match self {
            RoleAdjustment::ApRemovedNoWifi => "本设备没有无线模块，已取消 AP 角色并禁用该选项",
        }
    }
}

impl Roles {
    pub const ALL: [&'static str; 3] = ["controller", "ap", "gateway"];

    pub fn none() -> Self {
        Self::default()
    }

    pub fn controller_only() -> Self {
        Self {
            controller: true,
            ..Default::default()
        }
    }

    pub fn ap_only() -> Self {
        Self {
            ap: true,
            ..Default::default()
        }
    }

    /// 需求 3 的默认值：有无线 ⇒ 三种角色全开；无无线 ⇒ 只有 controller + gateway。
    pub fn default_for(caps: &Capabilities) -> Self {
        Self {
            controller: true,
            ap: caps.has_wifi(),
            gateway: true,
        }
    }

    pub fn has_any(&self) -> bool {
        self.controller || self.ap || self.gateway
    }

    pub fn count(&self) -> usize {
        [self.controller, self.ap, self.gateway]
            .iter()
            .filter(|v| **v)
            .count()
    }

    /// 是否允许「把所有网口组成一个网桥」。整份代码里唯一允许建桥的入口。
    pub fn enable_bridge(&self) -> bool {
        self.ap && !self.gateway && !self.controller
    }

    /// 是否承担 Gateway：Gateway 一律不动用户网络。
    pub fn is_gateway(&self) -> bool {
        self.gateway
    }

    /// 是否承担 Controller。
    pub fn is_controller(&self) -> bool {
        self.controller
    }

    pub fn labels(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.controller {
            out.push("controller");
        }
        if self.ap {
            out.push("ap");
        }
        if self.gateway {
            out.push("gateway");
        }
        out
    }

    /// 去掉硬件不支持的角色；返回被调整的项，供 UI 提示。
    pub fn sanitize(&mut self, caps: &Capabilities) -> Vec<RoleAdjustment> {
        let mut adjustments = Vec::new();
        if self.ap && !caps.has_wifi() {
            self.ap = false;
            adjustments.push(RoleAdjustment::ApRemovedNoWifi);
        }
        adjustments
    }

    /// 校验能否设置：AP 需要无线硬件。
    pub fn validate(&self, caps: &Capabilities) -> CoreResult<()> {
        if self.ap && !caps.has_wifi() {
            return Err(CoreError::ApRequiresWifi);
        }
        Ok(())
    }

    /// uci 存储形式，如 `"controller ap gateway"`。
    pub fn to_uci_value(&self) -> String {
        self.labels().join(" ")
    }

    pub fn from_uci_value(value: &str) -> CoreResult<Self> {
        let mut roles = Self::none();
        for token in value.split(|c: char| c.is_whitespace() || c == ',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match token {
                "controller" => roles.controller = true,
                "ap" => roles.ap = true,
                "gateway" => roles.gateway = true,
                other => return Err(CoreError::UnknownRole(other.to_string())),
            }
        }
        Ok(roles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Capabilities, RadioInfo};

    fn caps_with_wifi() -> Capabilities {
        Capabilities {
            radios: vec![RadioInfo {
                name: "radio0".into(),
                band: Some("5g".into()),
                channel: Some(36),
                supports_kvr: true,
            }],
            wpad_full: true,
            ..Default::default()
        }
    }

    #[test]
    fn default_roles_with_wifi_are_all_three() {
        let roles = Roles::default_for(&caps_with_wifi());
        assert!(roles.controller && roles.ap && roles.gateway);
    }

    #[test]
    fn default_roles_without_wifi_drop_ap() {
        let roles = Roles::default_for(&Capabilities::default());
        assert!(roles.controller && roles.gateway);
        assert!(!roles.ap, "无无线设备默认不得承担 AP");
    }

    #[test]
    fn sanitize_removes_ap_and_reports() {
        let mut roles = Roles {
            controller: true,
            ap: true,
            gateway: false,
        };
        let changes = roles.sanitize(&Capabilities::default());
        assert_eq!(changes, vec![RoleAdjustment::ApRemovedNoWifi]);
        assert!(!roles.ap);
    }

    #[test]
    fn only_ap_role_enables_bridge() {
        // 穷举 2^3 组合，只有 {ap} 允许建桥（R5）
        for bits in 0u8..8 {
            let roles = Roles {
                controller: bits & 0b001 != 0,
                ap: bits & 0b010 != 0,
                gateway: bits & 0b100 != 0,
            };
            let expected = roles.ap && !roles.gateway && !roles.controller;
            assert_eq!(roles.enable_bridge(), expected, "roles={:?}", roles);
        }
        assert!(Roles::ap_only().enable_bridge());
        assert!(!Roles {
            controller: true,
            ap: true,
            gateway: false
        }
        .enable_bridge());
    }

    #[test]
    fn validate_rejects_ap_without_wifi() {
        let roles = Roles::ap_only();
        assert_eq!(
            roles.validate(&Capabilities::default()),
            Err(CoreError::ApRequiresWifi)
        );
        assert!(roles.validate(&caps_with_wifi()).is_ok());
    }

    #[test]
    fn uci_roundtrip() {
        let roles = Roles {
            controller: true,
            ap: false,
            gateway: true,
        };
        assert_eq!(roles.to_uci_value(), "controller gateway");
        assert_eq!(Roles::from_uci_value("controller gateway").unwrap(), roles);
        assert_eq!(
            Roles::from_uci_value("controller,ap,gateway").unwrap(),
            Roles {
                controller: true,
                ap: true,
                gateway: true
            }
        );
        assert!(Roles::from_uci_value("bogus").is_err());
    }
}
