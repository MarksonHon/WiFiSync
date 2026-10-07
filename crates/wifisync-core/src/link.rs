//! Controller link: the model shared by the Controller (server) and the AP / Gateway (clients).
//!
//! The Controller is the forwarding centre. AP and Gateway nodes connect to it, authenticate with
//! a Controller account (except over loopback, when they share the device with the Controller),
//! and periodically `sync`:
//!
//! * the Gateway reports its LAN area ([`crate::lan::LanReport`]);
//! * the AP reports itself for admission and receives the network profile plus the Gateway LAN
//!   information the Controller collected.
//!
//! This module only holds data, limits and validation; the wire protocol lives in the `wifisync`
//! binary.

use crate::admission::AdmissionState;
use crate::lan::LanReport;
use crate::profile::NetworkProfile;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Default Controller port; the user may change it (`controller_port`).
pub const DEFAULT_PORT: u16 = 6550;
/// Default listen address of the Controller.
pub const DEFAULT_BIND: &str = "0.0.0.0";
pub const PROTOCOL_VERSION: u32 = 1;

pub const MIN_PASSWORD_LEN: usize = 8;
pub const MAX_PASSWORD_LEN: usize = 128;
pub const MAX_USERNAME_LEN: usize = 32;
pub const MAX_DEVICE_ID_LEN: usize = 64;
const MAX_NAME_LEN: usize = 64;

/// What a connecting node is acting as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Ap,
    Gateway,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub device_id: String,
    pub roles: Vec<NodeRole>,
    #[serde(default)]
    pub mac: String,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub radios: u32,
}

impl NodeInfo {
    pub fn has_role(&self, role: NodeRole) -> bool {
        self.roles.contains(&role)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRequest {
    pub node: NodeInfo,
    /// Version of the profile the node already has (0 = none).
    #[serde(default)]
    pub profile_version: u64,
    /// Gateway role only: the current LAN report.
    #[serde(default)]
    pub lan: Option<LanReport>,
}

/// A LAN report as stored by the Controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayLan {
    pub device_id: String,
    pub hostname: Option<String>,
    pub source_addr: Option<String>,
    pub reported_at: crate::Timestamp,
    pub report: LanReport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncResponse {
    pub server_time: crate::Timestamp,
    /// Set for nodes with the AP role.
    #[serde(default)]
    pub admission: Option<AdmissionState>,
    /// Present only for an admitted AP whose profile is older than the Controller's.
    #[serde(default)]
    pub profile: Option<NetworkProfile>,
    /// Gateway LAN information forwarded to an admitted AP.
    #[serde(default)]
    pub lan: Vec<GatewayLan>,
    /// Secrets the node must store (today: the Wi-Fi key of the profile above).
    ///
    /// Deliberately **outside** [`NetworkProfile`]: the profile is published through LuCI
    /// (`profile.publish`), and a secret must never travel that way.
    #[serde(default)]
    pub secrets: Vec<SyncSecret>,
}

/// One secret shipped to a node.
///
/// The value is sealed on the wire (see the link module in the `wifisync` binary) and written to
/// `/etc/wifisync/secrets/` (0600, root only) on arrival. It is never logged, never rendered in a
/// dry-run text and never part of a write plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSecret {
    /// Secret-store reference; the node stores the value under this name.
    pub reference: String,
    pub value: String,
}

/// Longest accepted secret reference.
pub const MAX_SECRET_REF_LEN: usize = 64;

/// Application messages, one JSON object per frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Sync(SyncRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Response {
    Sync(SyncResponse),
    Error { message: String },
}

fn clean_text(value: &str, max: usize) -> bool {
    value.len() <= max && !value.chars().any(|c| c.is_control())
}

/// Identifiers that end up in paths, logs and the UI.
pub fn valid_identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// Whether a secret reference is acceptable. It becomes a file name in the secret store, so the
/// character set is restricted (the store sanitizes separators as well, but rejecting early keeps
/// the error visible to the caller).
pub fn valid_secret_reference(value: &str) -> bool {
    valid_identifier(value, MAX_SECRET_REF_LEN)
}

pub fn valid_username(value: &str) -> bool {
    valid_identifier(value, MAX_USERNAME_LEN)
}

pub fn check_password(value: &str) -> Result<(), String> {
    let len = value.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(format!(
            "the password must be at least {} characters",
            MIN_PASSWORD_LEN
        ));
    }
    if len > MAX_PASSWORD_LEN || value.chars().any(|c| c.is_control()) {
        return Err("the password is too long or contains control characters".to_string());
    }
    Ok(())
}

impl SyncRequest {
    /// The request comes from the network: bound every field before it is stored or displayed.
    pub fn validate(&self) -> Result<(), String> {
        let node = &self.node;
        if !valid_identifier(&node.device_id, MAX_DEVICE_ID_LEN) {
            return Err("invalid device_id".to_string());
        }
        if node.roles.is_empty() {
            return Err("the node reports no role".to_string());
        }
        if !clean_text(&node.mac, 32)
            || !node
                .hostname
                .as_deref()
                .map_or(true, |v| clean_text(v, MAX_NAME_LEN))
            || !node
                .model
                .as_deref()
                .map_or(true, |v| clean_text(v, MAX_NAME_LEN))
        {
            return Err("invalid node description".to_string());
        }
        if node.has_role(NodeRole::Gateway) {
            if let Some(lan) = &self.lan {
                lan.validate()?;
            }
        } else if self.lan.is_some() {
            return Err("only a node with the Gateway role may report a LAN".to_string());
        }
        Ok(())
    }
}

/// Loopback check that also sees IPv4-mapped IPv6 addresses (`::ffff:127.0.0.1`).
pub fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// Normalize a user supplied Controller address to `host:port` (`[v6]:port` for IPv6).
pub fn endpoint_with_port(endpoint: &str, default_port: u16) -> String {
    let endpoint = endpoint.trim();
    if let Some(rest) = endpoint.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((_, tail)) if tail.starts_with(':') => endpoint.to_string(),
            Some((host, _)) => format!("[{}]:{}", host, default_port),
            None => endpoint.to_string(),
        };
    }
    match endpoint.matches(':').count() {
        0 => format!("{}:{}", endpoint, default_port),
        1 => endpoint.to_string(),
        // Bare IPv6 literal
        _ => format!("[{}]:{}", endpoint, default_port),
    }
}

/// Validate a listen address (an IP literal, so no name resolution is needed).
pub fn check_bind(value: &str) -> Result<(), String> {
    value
        .parse::<IpAddr>()
        .map(|_| ())
        .map_err(|_| format!("`{}` is not a valid listen address", value))
}

pub fn check_port(value: u64) -> Result<u16, String> {
    match u16::try_from(value) {
        Ok(port) if port >= 1 => Ok(port),
        _ => Err("the port must be between 1 and 65535".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(roles: Vec<NodeRole>) -> NodeInfo {
        NodeInfo {
            device_id: "dev-1".into(),
            roles,
            ..NodeInfo::default()
        }
    }

    #[test]
    fn loopback_detection_covers_mapped_addresses() {
        assert!(is_loopback("127.0.0.1".parse().unwrap()));
        assert!(is_loopback("127.5.6.7".parse().unwrap()));
        assert!(is_loopback("::1".parse().unwrap()));
        assert!(is_loopback("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_loopback("192.168.1.1".parse().unwrap()));
        assert!(!is_loopback("::ffff:192.168.1.1".parse().unwrap()));
        assert!(!is_loopback("fe80::1".parse().unwrap()));
    }

    #[test]
    fn endpoints_get_the_default_port() {
        assert_eq!(endpoint_with_port("192.168.1.1", 6550), "192.168.1.1:6550");
        assert_eq!(
            endpoint_with_port("router.lan:7000", 6550),
            "router.lan:7000"
        );
        assert_eq!(endpoint_with_port("fd00::1", 6550), "[fd00::1]:6550");
        assert_eq!(endpoint_with_port("[fd00::1]", 6550), "[fd00::1]:6550");
        assert_eq!(endpoint_with_port("[fd00::1]:99", 6550), "[fd00::1]:99");
    }

    #[test]
    fn port_and_bind_validation() {
        assert_eq!(check_port(6550), Ok(6550));
        assert!(check_port(0).is_err());
        assert!(check_port(70000).is_err());
        assert!(check_bind("0.0.0.0").is_ok());
        assert!(check_bind("::").is_ok());
        assert!(check_bind("router.lan").is_err());
    }

    #[test]
    fn credentials_are_validated() {
        assert!(valid_username("ap-1.lan"));
        assert!(!valid_username(""));
        assert!(!valid_username("a b"));
        assert!(!valid_username("../etc"));
        assert!(check_password("12345678").is_ok());
        assert!(check_password("short").is_err());
        assert!(check_password(&"x".repeat(MAX_PASSWORD_LEN + 1)).is_err());
    }

    #[test]
    fn sync_request_validation() {
        let mut request = SyncRequest {
            node: node(vec![NodeRole::Ap]),
            ..SyncRequest::default()
        };
        assert!(request.validate().is_ok());

        request.node.device_id = "../x".into();
        assert!(request.validate().is_err());
        request.node.device_id = "dev-1".into();

        request.node.roles.clear();
        assert!(request.validate().is_err());
        request.node.roles = vec![NodeRole::Ap];

        request.lan = Some(LanReport::default());
        assert!(request.validate().is_err(), "an AP must not report a LAN");
        request.node.roles = vec![NodeRole::Ap, NodeRole::Gateway];
        assert!(request.validate().is_ok());
    }

    #[test]
    fn messages_roundtrip_through_json() {
        let request = Request::Sync(SyncRequest {
            node: node(vec![NodeRole::Gateway]),
            profile_version: 3,
            lan: Some(LanReport::default()),
        });
        let text = serde_json::to_string(&request).unwrap();
        assert!(text.contains("\"op\":\"sync\""));
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), request);

        let response = Response::Error {
            message: "nope".into(),
        };
        let text = serde_json::to_string(&response).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&text).unwrap(), response);
    }
}
