//! Role model (requirement 3 / 5 / 7 / 8).
//!
//! The three roles are **capability sets**, and one device can take on several at once:
//!
//! | Role | Default | Changes to the local network |
//! |------|---------|------------------------------|
//! | `gateway` | on by default with wireless | **none** (only lets the user pick LAN interfaces for identification/probing) |
//! | `controller` | on by default with wireless | **none** (only new AP admission + information push) |
//! | `ap` | on by default with wireless; **forced off without wireless** | applies the pushed Wi-Fi / bridge information |
//!
//! Only a "pure AP" (ap set, and neither gateway nor controller) automatically merges all network
//! ports into `br-lan`.

use crate::capability::Capabilities;
use crate::error::{CoreError, CoreResult};
use crate::message::Message;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Roles {
    pub controller: bool,
    pub ap: bool,
    pub gateway: bool,
}

/// Reason a role was adjusted automatically, for the UI (requirement 3: drop the default AP role
/// and disable it when there is no wireless).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleAdjustment {
    ApRemovedNoWifi,
}

impl RoleAdjustment {
    /// UI message for this adjustment.
    pub fn message(&self) -> Message {
        match self {
            RoleAdjustment::ApRemovedNoWifi => Message::new("role.ap_removed_no_wifi"),
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

    /// Requirement 3 defaults: with wireless ⇒ all three roles on; without wireless ⇒ only
    /// controller + gateway.
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

    /// Whether "merge all network ports into one bridge" is allowed. The only entry point in the
    /// whole codebase that permits creating a bridge.
    pub fn enable_bridge(&self) -> bool {
        self.ap && !self.gateway && !self.controller
    }

    /// Whether this device takes the Gateway role: a Gateway never touches the user's network.
    pub fn is_gateway(&self) -> bool {
        self.gateway
    }

    /// Whether this device takes the Controller role.
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

    /// Remove roles the hardware does not support; returns the adjustments for the UI to show.
    pub fn sanitize(&mut self, caps: &Capabilities) -> Vec<RoleAdjustment> {
        let mut adjustments = Vec::new();
        if self.ap && !caps.has_wifi() {
            self.ap = false;
            adjustments.push(RoleAdjustment::ApRemovedNoWifi);
        }
        adjustments
    }

    /// Validate whether the roles can be set: AP requires wireless hardware.
    pub fn validate(&self, caps: &Capabilities) -> CoreResult<()> {
        if self.ap && !caps.has_wifi() {
            return Err(CoreError::ApRequiresWifi);
        }
        Ok(())
    }

    /// uci storage form, e.g. `"controller ap gateway"`.
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
        assert!(
            !roles.ap,
            "a device without wireless must not default to AP"
        );
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
        // Enumerate all 2^3 combinations; only {ap} may create a bridge (R5)
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
