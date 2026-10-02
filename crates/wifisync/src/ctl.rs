//! Command line interface.
//!
//! Design: **it must work offline**. If the daemon runs, commands use the UNIX socket;
//! otherwise build a [`Daemon`] in place and run it directly (backup/restore/plan need no daemon).

use crate::daemon::{Daemon, DaemonOptions};
use crate::rpc;
use serde_json::{json, Value};
use wifisync_sys::error::SysResult;
use wifisync_sys::Paths;

/// Convenience: forward (method, params) to the running service, or run once locally.
pub fn invoke(paths: &Paths, method: &str, params: Value) -> Result<Value, String> {
    let socket = paths.socket_file();
    if rpc::is_running(&socket) {
        return rpc::call(&socket, method, params);
    }
    let daemon = Daemon::new(paths.clone(), DaemonOptions::default())
        .map_err(|e| format!("initialization failed: {}", e))?;
    daemon.handle(method, &params)
}

pub fn status(paths: &Paths) -> Result<Value, String> {
    invoke(paths, "status", json!({}))
}

pub fn plan(paths: &Paths) -> Result<Value, String> {
    invoke(paths, "plan.dry_run", json!({}))
}

pub fn apply(paths: &Paths) -> Result<Value, String> {
    invoke(paths, "apply", json!({}))
}

pub fn confirm(paths: &Paths) -> Result<Value, String> {
    invoke(paths, "confirm", json!({}))
}

pub fn revert(paths: &Paths) -> Result<Value, String> {
    invoke(paths, "revert_last_change", json!({}))
}

pub fn restore(paths: &Paths, mode_full: bool, snapshot: Option<String>) -> Result<Value, String> {
    invoke(
        paths,
        "restore",
        json!({
            "mode": if mode_full { "full" } else { "managed_only" },
            "snapshot": snapshot,
        }),
    )
}

pub fn backup(paths: &Paths, action: &str) -> Result<Value, String> {
    match action {
        "list" => invoke(paths, "backup.list", json!({})),
        "verify" => invoke(paths, "backup.verify", json!({})),
        "prune" => invoke(paths, "backup.prune", json!({})),
        "create" | "" => invoke(paths, "backup.create", json!({ "kind": "pre-start" })),
        other => Err(format!("unknown backup subcommand `{}`", other)),
    }
}

pub fn admit(paths: &Paths, action: &str, device_id: Option<&str>) -> Result<Value, String> {
    match action {
        "list" => invoke(paths, "admission.list", json!({})),
        "approve" | "reject" | "revoke" => {
            let id = device_id.ok_or_else(|| "missing device_id".to_string())?;
            invoke(
                paths,
                &format!("admission.{}", action),
                json!({ "device_id": id }),
            )
        }
        other => Err(format!("unknown admit subcommand `{}`", other)),
    }
}

pub fn roles(paths: &Paths, value: Option<&str>) -> Result<Value, String> {
    match value {
        None => invoke(paths, "roles.get", json!({})),
        Some(value) => invoke(paths, "roles.set", json!({ "roles": value })),
    }
}

pub fn probe(paths: &Paths, target: &str) -> Result<Value, String> {
    invoke(paths, "probe.connectivity", json!({ "target": target }))
}

/// Print JSON (for the shell / LuCI).
pub fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{}", text),
        Err(_) => println!("{}", value),
    }
}

/// Initialize the paths (`WIFISYNC_ROOT` can override them, for tests and offline drills).
pub fn default_paths() -> SysResult<Paths> {
    Ok(Paths::new())
}
