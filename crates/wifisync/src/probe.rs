//! Read-only connectivity probing (requirement 8: whether the Controller and the Gateway are
//! connected is confirmed explicitly by the user; the program only probes, and never modifies
//! routing / NAT / firewall).

use serde_json::{json, Value};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use wifisync_core::Message;
use wifisync_sys::exec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub reachable: bool,
    pub method: String,
    pub latency_ms: Option<u128>,
    /// UI message (translated by the front end).
    pub detail: Message,
}

impl ProbeResult {
    pub fn to_json(&self) -> Value {
        json!({
            "reachable": self.reachable,
            "method": self.method,
            "latency_ms": self.latency_ms,
            "detail": self.detail,
        })
    }
}

/// Parse a target of the form `host`, `host:port` or `[v6]:port`.
fn split_target(target: &str) -> (String, u16) {
    if let Some(stripped) = target.strip_prefix('[') {
        if let Some((host, rest)) = stripped.split_once(']') {
            let port = rest
                .strip_prefix(':')
                .and_then(|p| p.parse().ok())
                .unwrap_or(443);
            return (host.to_string(), port);
        }
    }
    match target.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
            (host.to_string(), port.parse().unwrap_or(443))
        }
        _ => (target.to_string(), 443),
    }
}

/// TCP connectivity probe (ICMP needs raw socket privileges, so TCP is used uniformly here).
pub fn tcp_probe(target: &str) -> ProbeResult {
    let (host, port) = split_target(target);
    let address = format!("{}:{}", host, port);
    let addrs: Vec<_> = match address.to_socket_addrs() {
        Ok(addrs) => addrs.collect(),
        Err(e) => {
            return ProbeResult {
                reachable: false,
                method: "tcp".into(),
                latency_ms: None,
                detail: Message::new("probe.unresolved")
                    .param("address", address)
                    .param("error", e.to_string()),
            }
        }
    };

    let start = Instant::now();
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok() {
            return ProbeResult {
                reachable: true,
                method: "tcp".into(),
                latency_ms: Some(start.elapsed().as_millis()),
                detail: Message::new("probe.reachable").param("address", addr.to_string()),
            };
        }
    }
    ProbeResult {
        reachable: false,
        method: "tcp".into(),
        latency_ms: None,
        detail: Message::new("probe.unreachable_tcp").param("address", address),
    }
}

/// Optional ICMP probe (when `ping` is available).
pub fn ping_once(target: &str) -> Option<ProbeResult> {
    let (host, _) = split_target(target);
    if !exec::has("ping") {
        return None;
    }
    let start = Instant::now();
    let out = exec::run("ping", &["-c", "1", "-W", "2", &host]).ok()?;
    Some(ProbeResult {
        reachable: out.success(),
        method: "icmp".into(),
        latency_ms: if out.success() {
            Some(start.elapsed().as_millis())
        } else {
            None
        },
        detail: Message::new("probe.icmp_raw").param(
            "output",
            out.stdout.lines().next().unwrap_or_default().to_string(),
        ),
    })
}

/// Combined probe: ICMP first (if available), then TCP.
pub fn probe(target: &str) -> ProbeResult {
    if let Some(result) = ping_once(target) {
        if result.reachable {
            return result;
        }
    }
    tcp_probe(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_host_and_port() {
        assert_eq!(split_target("192.168.1.1"), ("192.168.1.1".into(), 443));
        assert_eq!(
            split_target("192.168.1.1:8443"),
            ("192.168.1.1".into(), 8443)
        );
        assert_eq!(split_target("[fe80::1]:8080"), ("fe80::1".into(), 8080));
        assert_eq!(split_target("router.lan"), ("router.lan".into(), 443));
    }

    #[test]
    fn unroutable_target_is_unreachable() {
        // 192.0.2.0/24 is TEST-NET-1, unreachable in a normal environment
        let result = tcp_probe("192.0.2.1:9");
        assert!(!result.reachable);
    }
}
