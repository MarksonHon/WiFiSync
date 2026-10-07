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

/// Read a password: without echo on a terminal, one line from stdin otherwise.
fn read_password(prompt: &str) -> Result<String, String> {
    let tty = unsafe { libc::isatty(libc::STDIN_FILENO) } == 1;
    let mut saved: Option<libc::termios> = None;
    if tty {
        eprint!("{}", prompt);
        unsafe {
            let mut term: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut term) == 0 {
                saved = Some(term);
                term.c_lflag &= !libc::ECHO;
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term);
            }
        }
    }
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    if let Some(term) = saved {
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term) };
        eprintln!();
    }
    read.map_err(|e| format!("reading the password failed: {}", e))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// `wifisync account list|add <name>|passwd <name>|remove <name>`
pub fn account(paths: &Paths, action: &str, username: Option<&str>) -> Result<Value, String> {
    match action {
        "list" => invoke(paths, "account.list", json!({})),
        "add" | "passwd" => {
            let username =
                username.ok_or_else(|| format!("usage: wifisync account {} <name>", action))?;
            let password = read_password("Password: ")?;
            invoke(
                paths,
                &format!("account.{}", action),
                json!({ "username": username, "password": password }),
            )
        }
        "remove" => {
            let username =
                username.ok_or_else(|| "usage: wifisync account remove <name>".to_string())?;
            invoke(paths, "account.remove", json!({ "username": username }))
        }
        other => Err(format!("unknown account subcommand `{}`", other)),
    }
}

/// `wifisync secret list|set <reference>|remove <reference>`
///
/// The value is read without echo, is never passed as a command line argument and is never printed
/// back: the secret store is the only place a value lives.
pub fn secret(paths: &Paths, action: &str, reference: Option<&str>) -> Result<Value, String> {
    match action {
        "list" => invoke(paths, "secret.list", json!({})),
        "set" => {
            let reference =
                reference.ok_or_else(|| "usage: wifisync secret set <reference>".to_string())?;
            let value = read_password("Secret: ")?;
            invoke(
                paths,
                "secret.set",
                json!({ "reference": reference, "value": value }),
            )
        }
        "remove" => {
            let reference =
                reference.ok_or_else(|| "usage: wifisync secret remove <reference>".to_string())?;
            invoke(paths, "secret.remove", json!({ "reference": reference }))
        }
        other => Err(format!("unknown secret subcommand `{}`", other)),
    }
}

/// `wifisync link [status]` / `link set <key> <value>` / `link password [--clear]`
pub fn link(paths: &Paths, args: &[String]) -> Result<Value, String> {
    let arg = |index: usize| args.get(index).map(|s| s.as_str());
    match arg(0).unwrap_or("status") {
        "status" => Ok(json!({
            "settings": invoke(paths, "link.get", json!({}))?,
            "status": invoke(paths, "link.status", json!({}))?,
        })),
        "set" => {
            let (key, value) = (
                arg(1).ok_or_else(|| {
                    "usage: wifisync link set <endpoint|username|port|bind> <value>".to_string()
                })?,
                arg(2).ok_or_else(|| "missing value".to_string())?,
            );
            let params = match key {
                "endpoint" => json!({ "controller_endpoint": value }),
                "username" => json!({ "controller_username": value }),
                "bind" => json!({ "controller_bind": value }),
                "port" => json!({
                    "controller_port": value
                        .parse::<u64>()
                        .map_err(|_| format!("`{}` is not a valid port", value))?
                }),
                other => return Err(format!("unknown link setting `{}`", other)),
            };
            invoke(paths, "link.set", params)
        }
        "password" => {
            if arg(1) == Some("--clear") {
                return invoke(paths, "link.set", json!({ "clear_password": true }));
            }
            let password = read_password("Controller password: ")?;
            invoke(
                paths,
                "link.set",
                json!({ "controller_password": password }),
            )
        }
        other => Err(format!("unknown link subcommand `{}`", other)),
    }
}

/// `wifisync lan [report|list]`
pub fn lan(paths: &Paths, action: &str) -> Result<Value, String> {
    match action {
        "report" => invoke(paths, "lan.report", json!({})),
        "list" => invoke(paths, "lan.list", json!({})),
        other => Err(format!("unknown lan subcommand `{}`", other)),
    }
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
