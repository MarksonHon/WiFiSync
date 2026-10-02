//! rpcd exec plugin protocol implementation (LuCI <-> ubus <-> wifisync).
//!
//! rpcd invokes `/usr/libexec/rpcd/wifisync`:
//!
//! * `wifisync ubus list` -> prints the method table JSON to stdout;
//! * `wifisync ubus call <method>` -> reads the params JSON from stdin and writes the result JSON
//!   to stdout.
//!
//! This way, the LuCI side uses the standard `rpc.declare({ object: 'wifisync', method: ... })`
//! and no new protocol has to be introduced.

use serde_json::{json, Map, Value};
use wifisync_sys::Paths;

/// Method table: LuCI uses it to generate RPC declarations and argument validation.
pub fn method_table() -> Value {
    let mut methods = Map::new();
    let mut add = |name: &str, args: Value, description: &str| {
        methods.insert(
            name.to_string(),
            json!({ "args": args, "description": description }),
        );
    };

    add("status", json!({}), "overall status");
    add(
        "capabilities",
        json!({}),
        "hardware capability probe result",
    );
    add("roles_get", json!({}), "read the role set");
    add(
        "roles_set",
        json!({ "roles": "string" }),
        "set the role set",
    );
    add("bridge_preview", json!({}), "bridge planning preview");
    add("plan_dry_run", json!({}), "dry-run write plan");
    add("apply", json!({}), "apply the write plan");
    add(
        "confirm",
        json!({}),
        "confirm a successful apply (releases apply-guard)",
    );
    add(
        "revert_last_change",
        json!({}),
        "revert the most recent write",
    );
    add(
        "restore",
        json!({ "mode": "string", "snapshot": "string" }),
        "restore from a snapshot (initial baseline by default)",
    );
    add(
        "wifi_source_get",
        json!({}),
        "read the Wi-Fi information source",
    );
    add(
        "wifi_source_set",
        json!({
            "kind": "string",
            "gateway_endpoint": "string",
            "local_wifi_change_confirmed": "boolean",
            "custom": {},
        }),
        "set the Wi-Fi information source",
    );
    add(
        "gateway_set",
        json!({ "lan_ifaces": {} }),
        "record the Gateway LAN interfaces (does not modify the network)",
    );
    add(
        "probe_connectivity",
        json!({ "target": "string" }),
        "read-only connectivity probe",
    );
    add("admission_list", json!({}), "admission list");
    add(
        "admission_register",
        json!({ "device_id": "string", "mac": "string" }),
        "device report (AP side)",
    );
    add(
        "admission_approve",
        json!({ "device_id": "string" }),
        "approve a device",
    );
    add(
        "admission_reject",
        json!({ "device_id": "string" }),
        "reject a device",
    );
    add(
        "admission_revoke",
        json!({ "device_id": "string" }),
        "revoke an approval",
    );
    add("backup_list", json!({}), "snapshot list");
    add(
        "backup_verify",
        json!({ "path": "string" }),
        "verify snapshot integrity",
    );
    add(
        "backup_create",
        json!({ "kind": "string" }),
        "create a snapshot manually",
    );
    add(
        "backup_prune",
        json!({}),
        "prune snapshots according to the retention policy",
    );
    add(
        "failsafe_get",
        json!({}),
        "read the failover configuration and state",
    );
    add(
        "failsafe_set",
        json!({
            "enabled": "boolean",
            "link_timeout_secs": "integer",
            "apply_confirm_secs": "integer",
            "action": "string",
            "keep_ssid": "boolean",
            "heartbeat_endpoint": "string",
        }),
        "set the failover configuration",
    );
    add("profile_get", json!({}), "read the network profile");
    add(
        "profile_publish",
        json!({ "profile": {} }),
        "distribute the network profile",
    );
    add(
        "logs_tail",
        json!({ "lines": "integer" }),
        "read the tail of the service log",
    );
    add("version", json!({}), "version information");

    json!({ "wifisync": { "methods": methods } })
}

/// ubus method name (underscore style, as usual on the LuCI side) -> internal RPC method name
/// (dotted style).
///
/// An explicit mapping instead of a blind replacement, so `plan_dry_run` cannot be mismapped to
/// `plan.dry.run`.
pub fn to_rpc_method(name: &str) -> String {
    let mapped = match name {
        "plan_dry_run" => "plan.dry_run",
        "revert_last_change" => "revert_last_change",
        "backup_list" => "backup.list",
        "backup_verify" => "backup.verify",
        "backup_create" => "backup.create",
        "backup_prune" => "backup.prune",
        "admission_list" => "admission.list",
        "admission_register" => "admission.register",
        "admission_approve" => "admission.approve",
        "admission_reject" => "admission.reject",
        "admission_revoke" => "admission.revoke",
        "wifi_source_get" => "wifi_source.get",
        "wifi_source_set" => "wifi_source.set",
        "probe_connectivity" => "probe.connectivity",
        "failsafe_get" => "failsafe.get",
        "failsafe_set" => "failsafe.set",
        "profile_get" => "profile.get",
        "profile_publish" => "profile.publish",
        "roles_get" => "roles.get",
        "roles_set" => "roles.set",
        "bridge_preview" => "bridge.preview",
        "gateway_set" => "gateway.set",
        "logs_tail" => "logs.tail",
        // Unambiguous methods pass through unchanged
        other => other,
    };
    mapped.to_string()
}

/// Handle `wifisync ubus <subcommand>`.
pub fn run(paths: &Paths, args: &[String]) -> Result<i32, String> {
    match args.first().map(|s| s.as_str()) {
        Some("list") => {
            println!(
                "{}",
                serde_json::to_string(&method_table()).unwrap_or_else(|_| "{}".into())
            );
            Ok(0)
        }
        Some("call") => {
            let method = args
                .get(1)
                .ok_or_else(|| "usage: wifisync ubus call <method>".to_string())?;
            let params = read_stdin_json()?;
            let rpc_method = to_rpc_method(method);
            match crate::ctl::invoke(paths, &rpc_method, params) {
                Ok(result) => {
                    println!(
                        "{}",
                        serde_json::to_string(&result).unwrap_or_else(|_| "null".into())
                    );
                    Ok(0)
                }
                Err(error) => {
                    // rpcd convention: on failure, write the reason to stderr and exit non-zero
                    eprintln!("{}", error);
                    Ok(1)
                }
            }
        }
        _ => Err("usage: wifisync ubus [list|call <method>]".to_string()),
    }
}

fn read_stdin_json() -> Result<Value, String> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .map_err(|e| format!("reading stdin failed: {}", e))?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(trimmed).map_err(|e| format!("arguments are not valid JSON: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_table_exposes_core_methods() {
        let table = method_table();
        let methods = &table["wifisync"]["methods"];
        for name in [
            "status",
            "roles_set",
            "plan_dry_run",
            "apply",
            "restore",
            "admission_approve",
            "backup_verify",
            "failsafe_set",
            "wifi_source_set",
        ] {
            assert!(methods.get(name).is_some(), "missing method {}", name);
        }
    }

    #[test]
    fn method_names_map_to_rpc_dotted_form() {
        assert_eq!(to_rpc_method("roles_get"), "roles.get");
        assert_eq!(to_rpc_method("plan_dry_run"), "plan.dry_run");
        assert_eq!(to_rpc_method("admission_approve"), "admission.approve");
        assert_eq!(to_rpc_method("probe_connectivity"), "probe.connectivity");
        // Unambiguous methods pass through unchanged
        assert_eq!(to_rpc_method("apply"), "apply");
        assert_eq!(to_rpc_method("status"), "status");
    }

    /// Every ubus method in the method table must map to an internal method that really exists.
    #[test]
    fn every_declared_method_is_dispatchable() {
        let known_internal = [
            "status",
            "capabilities",
            "roles.get",
            "roles.set",
            "bridge.preview",
            "plan.dry_run",
            "apply",
            "confirm",
            "revert_last_change",
            "restore",
            "wifi_source.get",
            "wifi_source.set",
            "gateway.set",
            "probe.connectivity",
            "admission.list",
            "admission.register",
            "admission.approve",
            "admission.reject",
            "admission.revoke",
            "backup.list",
            "backup.verify",
            "backup.create",
            "backup.prune",
            "failsafe.get",
            "failsafe.set",
            "profile.get",
            "profile.publish",
            "logs.tail",
            "version",
        ];
        let table = method_table();
        let methods = table["wifisync"]["methods"].as_object().unwrap();
        for name in methods.keys() {
            let mapped = to_rpc_method(name);
            assert!(
                known_internal.contains(&mapped.as_str()),
                "ubus method {} maps to an unknown internal method {}",
                name,
                mapped
            );
        }
    }
}
