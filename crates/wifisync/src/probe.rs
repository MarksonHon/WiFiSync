//! 只读连通性探测（需求 8：Controller 与 Gateway 是否连通由用户显式确认，
//! 程序只做探测，绝不修改路由 / NAT / 防火墙）。

use serde_json::{json, Value};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use wifisync_sys::exec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub reachable: bool,
    pub method: String,
    pub latency_ms: Option<u128>,
    pub detail: String,
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

/// 解析 `host`、`host:port`、`[v6]:port` 形式的目标。
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

/// TCP 连接探测（ICMP 需要 raw socket 权限，这里统一用 TCP）。
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
                detail: format!("无法解析 {}: {}", address, e),
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
                detail: format!("{} 可达", addr),
            };
        }
    }
    ProbeResult {
        reachable: false,
        method: "tcp".into(),
        latency_ms: None,
        detail: format!("{} 不可达（TCP 连接失败）", address),
    }
}

/// 可选的 ICMP 探测（`ping` 存在时）。
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
        detail: out.stdout.lines().next().unwrap_or_default().to_string(),
    })
}

/// 综合探测：先 ICMP（若可用），再 TCP。
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
        // 192.0.2.0/24 是 TEST-NET-1，正常环境不可达
        let result = tcp_probe("192.0.2.1:9");
        assert!(!result.reachable);
    }
}
