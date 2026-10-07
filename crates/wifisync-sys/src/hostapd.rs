//! hostapd `ubus` adapter: read connected stations, switch on 802.11k/v capabilities, request a
//! client to move, and subscribe to hostapd notifications.
//!
//! Steering is implemented inside `wifisync` instead of depending on an external daemon
//! (`usteer` / `DAWN`), so this module is the only place that talks to hostapd. Everything goes
//! through the `ubus` CLI; the binary blob protocol is deliberately not reimplemented.
//!
//! hostapd exports one ubus object per BSS: `hostapd.<ifname>` (for example `hostapd.wlan0`).
//!
//! ⚠️ The reply schemas on the hostapd side are not documented upstream. Parsing here is therefore
//! tolerant — unknown keys are ignored and every value that a build may or may not report is an
//! `Option` — and the field names live in `const`s so that they can be adjusted in one place once
//! the S0 checks in `docs/STEERING.md` §13 have run against a real target.

use crate::error::{SysError, SysResult};
use crate::exec;
use serde_json::{json, Map, Value};
use std::process::{Child, Command, Stdio};

/// The `ubus` CLI defaults to a 30 s timeout; daemon calls must be bounded much tighter than that.
const UBUS_TIMEOUT_SECS: &str = "5";

/// Field holding the RSSI value of a station inside a `get_clients` entry. ⚠️ Verify in S0.
const CLIENT_SIGNAL_FIELD: &str = "signal";
/// Field marking an associated station inside a `get_clients` entry. ⚠️ Verify in S0.
const CLIENT_ASSOC_FIELD: &str = "assoc";
/// Field marking an authorized station inside a `get_clients` entry. ⚠️ Verify in S0.
const CLIENT_AUTHORIZED_FIELD: &str = "authorized";

/// ubus object path of the BSS served by `iface`.
pub fn hostapd_object(iface: &str) -> String {
    format!("hostapd.{}", iface)
}

/// Invoke a hostapd method and return its raw JSON reply (`Value::Null` for an empty reply).
///
/// Public on purpose: while the hostapd schemas are still unconfirmed, the raw reply *is* the
/// evidence used to pin down what [`station_list`] parses.
pub fn call_raw(iface: &str, method: &str, payload: Option<&Value>) -> SysResult<Value> {
    ubus_call(&hostapd_object(iface), method, payload)
}

fn ubus_call(object: &str, method: &str, payload: Option<&Value>) -> SysResult<Value> {
    let body = payload.cloned().unwrap_or_else(|| json!({})).to_string();
    let out = exec::run(
        "ubus",
        &["-S", "-t", UBUS_TIMEOUT_SECS, "call", object, method, &body],
    )?;
    if !out.success() {
        return Err(SysError::Command {
            program: format!("ubus call {} {}", object, method),
            message: format!("exit code {} / stderr: {}", out.status, out.stderr.trim()),
        });
    }
    let text = out.stdout.trim();
    if text.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(text)?)
}

/// One station as reported by hostapd for a BSS.
///
/// Signal and frequency stay optional: a build that reports less must degrade to "no opinion"
/// rather than fail the whole read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientReading {
    /// MAC address, lower-cased.
    pub mac: String,
    /// RSSI in dBm, when the build reports it.
    pub signal_dbm: Option<i32>,
    /// Operating frequency in MHz, when the build reports it.
    pub freq: Option<u32>,
    /// Whether the station is currently associated and authorized.
    pub connected: bool,
}

/// Read the stations connected to the BSS served by `iface`.
pub fn station_list(iface: &str) -> SysResult<Vec<ClientReading>> {
    let reply = call_raw(iface, "get_clients", None)?;
    parse_clients(&reply, iface)
}

/// Parse a `get_clients` reply of the shape `{ "freq": <int>, "clients": { "<mac>": { .. } } }`.
pub fn parse_clients(reply: &Value, iface: &str) -> SysResult<Vec<ClientReading>> {
    let clients = reply
        .get("clients")
        .and_then(Value::as_object)
        .ok_or_else(|| SysError::Parse {
            what: format!("get_clients on {}", hostapd_object(iface)),
            message: "reply carries no `clients` object".to_string(),
        })?;

    let freq = reply.get("freq").and_then(Value::as_u64).map(|v| v as u32);
    let mut stations = Vec::with_capacity(clients.len());
    for (mac, entry) in clients {
        let connected = entry
            .get(CLIENT_ASSOC_FIELD)
            .and_then(Value::as_bool)
            .unwrap_or(true)
            && entry
                .get(CLIENT_AUTHORIZED_FIELD)
                .and_then(Value::as_bool)
                .unwrap_or(true);
        stations.push(ClientReading {
            mac: mac.to_ascii_lowercase(),
            signal_dbm: entry
                .get(CLIENT_SIGNAL_FIELD)
                .and_then(Value::as_i64)
                .map(|v| v as i32),
            freq,
            connected,
        });
    }
    stations.sort_by(|a, b| a.mac.cmp(&b.mac));
    Ok(stations)
}

/// 802.11k/v capabilities that can be switched on at runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BssMgmtFlags {
    pub neighbor_report: bool,
    pub beacon_report: bool,
    pub link_measurement: bool,
    pub bss_transition: bool,
}

impl BssMgmtFlags {
    /// Everything steering needs: neighbor and beacon reports (802.11k) plus BTM (802.11v).
    pub const STEERING: Self = Self {
        neighbor_report: true,
        beacon_report: true,
        link_measurement: true,
        bss_transition: true,
    };

    /// Whether at least one capability is requested.
    pub fn any(&self) -> bool {
        self.neighbor_report || self.beacon_report || self.link_measurement || self.bss_transition
    }

    /// Only the requested capabilities are sent: hostapd reads these arguments by presence, so an
    /// explicit `false` is deliberately never emitted.
    fn to_payload(self) -> Value {
        let mut map = Map::new();
        if self.neighbor_report {
            map.insert("neighbor_report".to_string(), json!(true));
        }
        if self.beacon_report {
            map.insert("beacon_report".to_string(), json!(true));
        }
        // Singular `link_measurement` is what hostapd's ubus policy accepts; the plural spelling
        // used in its README is not recognised by the code.
        if self.link_measurement {
            map.insert("link_measurement".to_string(), json!(true));
        }
        if self.bss_transition {
            map.insert("bss_transition".to_string(), json!(true));
        }
        Value::Object(map)
    }
}

/// Switch on 802.11k/v capabilities for the BSS served by `iface`.
///
/// This is runtime state and has to be re-issued after every hostapd restart, which is why it is
/// meant to be paired with [`wait_for`].
pub fn bss_mgmt_enable(iface: &str, flags: BssMgmtFlags) -> SysResult<()> {
    if !flags.any() {
        return Ok(());
    }
    call_raw(iface, "bss_mgmt_enable", Some(&flags.to_payload())).map(|_| ())
}

/// An 802.11v BSS Transition Management request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BssTransitionRequest {
    pub mac: String,
    pub dialog_token: u8,
    /// Announce that the client is disconnected if it does not move.
    pub disassociation_imminent: bool,
    pub disassociation_timer: u32,
    pub reassoc_delay_ms: u32,
    pub abridged: bool,
    pub validity_period: u32,
    /// MBO reason code; omitted while zero.
    pub mbo_reason: u32,
    /// Candidate BSSIDs; while empty the request carries no candidate list.
    pub neighbors: Vec<String>,
}

impl BssTransitionRequest {
    /// A polite request: no disassociation is announced and nothing is enforced. This is the
    /// default steering mode.
    pub fn polite(mac: impl Into<String>, validity_period: u32) -> Self {
        Self {
            mac: mac.into(),
            dialog_token: 0,
            disassociation_imminent: false,
            disassociation_timer: 0,
            reassoc_delay_ms: 0,
            abridged: true,
            validity_period,
            mbo_reason: 0,
            neighbors: Vec::new(),
        }
    }

    fn to_payload(&self) -> Value {
        let mut map = Map::new();
        map.insert("addr".to_string(), json!(self.mac));
        map.insert("dialog_token".to_string(), json!(self.dialog_token));
        map.insert(
            "disassociation_imminent".to_string(),
            json!(self.disassociation_imminent),
        );
        if self.disassociation_imminent {
            map.insert(
                "disassociation_timer".to_string(),
                json!(self.disassociation_timer),
            );
            map.insert("reassoc_delay".to_string(), json!(self.reassoc_delay_ms));
        }
        map.insert("abridged".to_string(), json!(self.abridged));
        map.insert("validity_period".to_string(), json!(self.validity_period));
        if self.mbo_reason > 0 {
            map.insert("mbo_reason".to_string(), json!(self.mbo_reason));
        }
        if !self.neighbors.is_empty() {
            map.insert("neighbors".to_string(), json!(self.neighbors));
        }
        Value::Object(map)
    }
}

/// Ask the client `request.mac` to move to another BSS of the same ESS.
pub fn bss_transition_request(iface: &str, request: &BssTransitionRequest) -> SysResult<()> {
    call_raw(iface, "bss_transition_request", Some(&request.to_payload())).map(|_| ())
}

/// A single hostapd notification, for example `{ "assoc": { "address": "<mac>" } }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostapdEvent {
    /// Notification name (event type), for example `assoc`, `disassoc` or `probe`.
    pub method: String,
    pub data: Value,
}

/// Arguments of the notification subscriber for one BSS (without the program name).
pub fn subscriber_args(iface: &str) -> Vec<String> {
    vec!["subscribe".to_string(), hostapd_object(iface)]
}

/// Parse one line printed by `ubus subscribe`, which is `{ "<method>": <data> }`.
pub fn parse_subscriber_line(line: &str) -> SysResult<HostapdEvent> {
    let line = line.trim();
    let value: Value = serde_json::from_str(line)?;
    let object = value.as_object().ok_or_else(|| SysError::Parse {
        what: "ubus subscribe line".to_string(),
        message: format!("expected a JSON object, got `{}`", line),
    })?;

    let mut entries = object.iter();
    match (entries.next(), entries.next()) {
        (Some((method, data)), None) => Ok(HostapdEvent {
            method: method.clone(),
            data: data.clone(),
        }),
        _ => Err(SysError::Parse {
            what: "ubus subscribe line".to_string(),
            message: format!("expected exactly one key, got `{}`", line),
        }),
    }
}

/// Spawn the notification subscriber for one BSS.
///
/// The caller owns the process: it must keep reading `stdout` (one flushed JSON object per line,
/// see [`parse_subscriber_line`]) and respawn it when the child exits — a hostapd restart drops the
/// ubus object and with it the subscription, so [`wait_for`] is the intended way to resynchronize.
pub fn spawn_subscriber(iface: &str) -> SysResult<Child> {
    let args = subscriber_args(iface);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    Command::new("ubus")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => SysError::MissingProgram("ubus".to_string()),
            _ => SysError::Command {
                program: "ubus".to_string(),
                message: e.to_string(),
            },
        })
}

/// Block until every named ubus object exists, or the timeout expires.
///
/// Used before subscribing so that a hostapd restart does not become a busy retry loop.
pub fn wait_for(objects: &[&str], timeout_secs: u32) -> SysResult<()> {
    if objects.is_empty() {
        return Ok(());
    }
    let timeout = timeout_secs.to_string();
    let mut args = vec!["-t", timeout.as_str(), "wait_for"];
    args.extend_from_slice(objects);
    exec::run_ok("ubus", &args).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_path_follows_hostapd_naming() {
        assert_eq!(hostapd_object("wlan0"), "hostapd.wlan0");
        assert_eq!(subscriber_args("wlan0"), vec!["subscribe", "hostapd.wlan0"]);
    }

    #[test]
    fn clients_are_read_with_signal_and_frequency() {
        let reply = json!({
            "freq": 5180,
            "clients": {
                "AA:BB:CC:DD:EE:01": { "signal": -61, "assoc": true, "authorized": true },
                "aa:bb:cc:dd:ee:02": { "signal": -74, "assoc": true, "authorized": true }
            }
        });

        let stations = parse_clients(&reply, "wlan0").unwrap();

        assert_eq!(stations.len(), 2);
        // Sorted and lower-cased, so the caller sees a stable order.
        assert_eq!(stations[0].mac, "aa:bb:cc:dd:ee:01");
        assert_eq!(stations[0].signal_dbm, Some(-61));
        assert_eq!(stations[0].freq, Some(5180));
        assert!(stations[0].connected);
        assert_eq!(stations[1].signal_dbm, Some(-74));
    }

    #[test]
    fn a_station_that_is_not_associated_is_marked_disconnected() {
        let reply = json!({
            "clients": {
                "aa:bb:cc:dd:ee:03": { "signal": -50, "assoc": false, "authorized": false },
                "aa:bb:cc:dd:ee:04": { "signal": -52 }
            }
        });

        let stations = parse_clients(&reply, "wlan0").unwrap();

        assert!(!stations[0].connected);
        // Missing flags mean "unknown", which must not read as disconnected.
        assert!(stations[1].connected);
    }

    #[test]
    fn unknown_fields_and_missing_values_are_tolerated() {
        let reply = json!({
            "freq": 2412,
            "clients": {
                "aa:bb:cc:dd:ee:05": { "signal": "strong", "capab": 0x431, "bytes": {} }
            }
        });

        let stations = parse_clients(&reply, "wlan0").unwrap();

        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].signal_dbm, None);
        assert_eq!(stations[0].freq, Some(2412));
    }

    #[test]
    fn a_reply_without_a_clients_table_is_a_parse_error() {
        let err = parse_clients(&json!({ "freq": 2412 }), "wlan0").unwrap_err();
        assert!(matches!(err, SysError::Parse { .. }));
    }

    #[test]
    fn bss_mgmt_payload_uses_the_singular_link_measurement_key() {
        let payload = BssMgmtFlags::STEERING.to_payload();

        assert_eq!(payload["neighbor_report"], json!(true));
        assert_eq!(payload["beacon_report"], json!(true));
        assert_eq!(payload["bss_transition"], json!(true));
        assert_eq!(payload["link_measurement"], json!(true));
        assert!(payload.get("link_measurements").is_none());
    }

    #[test]
    fn bss_mgmt_payload_omits_everything_that_is_not_requested() {
        let flags = BssMgmtFlags {
            bss_transition: true,
            ..Default::default()
        };

        let payload = flags.to_payload();

        assert_eq!(payload.as_object().map(|o| o.len()), Some(1));
        assert_eq!(payload["bss_transition"], json!(true));
        assert!(payload.get("neighbor_report").is_none());
        assert!(!BssMgmtFlags::default().any());
    }

    #[test]
    fn polite_transition_request_announces_no_disassociation() {
        let request = BssTransitionRequest::polite("aa:bb:cc:dd:ee:01", 100);
        let payload = request.to_payload();

        assert_eq!(payload["addr"], json!("aa:bb:cc:dd:ee:01"));
        assert_eq!(payload["disassociation_imminent"], json!(false));
        assert_eq!(payload["validity_period"], json!(100));
        assert!(payload.get("disassociation_timer").is_none());
        assert!(payload.get("reassoc_delay").is_none());
        // No candidate list and no MBO reason while they are not set.
        assert!(payload.get("neighbors").is_none());
        assert!(payload.get("mbo_reason").is_none());
    }

    #[test]
    fn an_enforced_transition_request_carries_timer_candidates_and_reason() {
        let request = BssTransitionRequest {
            mac: "aa:bb:cc:dd:ee:01".to_string(),
            dialog_token: 7,
            disassociation_imminent: true,
            disassociation_timer: 1000,
            reassoc_delay_ms: 100,
            abridged: false,
            validity_period: 60,
            mbo_reason: 5,
            neighbors: vec!["11:22:33:44:55:66".to_string()],
        };
        let payload = request.to_payload();

        assert_eq!(payload["disassociation_imminent"], json!(true));
        assert_eq!(payload["disassociation_timer"], json!(1000));
        assert_eq!(payload["reassoc_delay"], json!(100));
        assert_eq!(payload["dialog_token"], json!(7));
        assert_eq!(payload["mbo_reason"], json!(5));
        assert_eq!(payload["neighbors"], json!(["11:22:33:44:55:66"]));
    }

    #[test]
    fn subscriber_lines_are_parsed_into_method_and_data() {
        let event =
            parse_subscriber_line("{\"assoc\": {\"address\": \"aa:bb:cc:dd:ee:01\"}}").unwrap();

        assert_eq!(event.method, "assoc");
        assert_eq!(event.data["address"], json!("aa:bb:cc:dd:ee:01"));
    }

    #[test]
    fn subscriber_lines_must_be_a_single_key_object() {
        assert!(parse_subscriber_line("").is_err());
        assert!(parse_subscriber_line("[]").is_err());
        assert!(parse_subscriber_line("{\"assoc\": {}, \"probe\": {}}").is_err());
    }
}
