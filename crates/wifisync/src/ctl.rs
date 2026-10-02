//! 命令行界面。
//!
//! 设计要点：**离线也能干活**。如果守护进程在跑，命令走 UNIX socket；
//! 否则就地构造一个 [`Daemon`] 实例直接执行（备份/恢复/计划这类操作本来就不需要常驻）。

use crate::daemon::{Daemon, DaemonOptions};
use crate::rpc;
use serde_json::{json, Value};
use wifisync_sys::error::SysResult;
use wifisync_sys::Paths;

/// 简化：把 (方法, 参数) 转发给运行中的服务，或本地一次性执行。
pub fn invoke(paths: &Paths, method: &str, params: Value) -> Result<Value, String> {
    let socket = paths.socket_file();
    if rpc::is_running(&socket) {
        return rpc::call(&socket, method, params);
    }
    let daemon = Daemon::new(paths.clone(), DaemonOptions::default())
        .map_err(|e| format!("初始化失败：{}", e))?;
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
        other => Err(format!("未知的 backup 子命令 `{}`", other)),
    }
}

pub fn admit(paths: &Paths, action: &str, device_id: Option<&str>) -> Result<Value, String> {
    match action {
        "list" => invoke(paths, "admission.list", json!({})),
        "approve" | "reject" | "revoke" => {
            let id = device_id.ok_or_else(|| "缺少 device_id".to_string())?;
            invoke(
                paths,
                &format!("admission.{}", action),
                json!({ "device_id": id }),
            )
        }
        other => Err(format!("未知的 admit 子命令 `{}`", other)),
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

/// 打印 JSON（供 shell / LuCI 使用）。
pub fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{}", text),
        Err(_) => println!("{}", value),
    }
}

/// 初始化路径（`WIFISYNC_ROOT` 可覆盖，便于测试与离线演练）。
pub fn default_paths() -> SysResult<Paths> {
    Ok(Paths::new())
}
