//! rpcd exec 插件协议实现（LuCI ↔ ubus ↔ wifisync）。
//!
//! rpcd 调用 `/usr/libexec/rpcd/wifisync`：
//!
//! * `wifisync ubus list` → 在 stdout 打印方法表 JSON；
//! * `wifisync ubus call <method>` → 从 stdin 读取参数 JSON，把结果 JSON 打到 stdout。
//!
//! 这样 LuCI 侧就是标准的 `rpc.declare({ object: 'wifisync', method: ... })`，
//! 不需要引入任何新协议。

use serde_json::{json, Map, Value};
use wifisync_sys::Paths;

/// 方法表：LuCI 用它生成 RPC 声明与参数校验。
pub fn method_table() -> Value {
    let mut methods = Map::new();
    let mut add = |name: &str, args: Value, description: &str| {
        methods.insert(
            name.to_string(),
            json!({ "args": args, "description": description }),
        );
    };

    add("status", json!({}), "总览状态");
    add("capabilities", json!({}), "硬件能力探测结果");
    add("roles_get", json!({}), "读取角色集合");
    add("roles_set", json!({ "roles": "string" }), "设置角色集合");
    add("bridge_preview", json!({}), "网桥规划预览");
    add("plan_dry_run", json!({}), "dry-run 写入计划");
    add("apply", json!({}), "应用写入计划");
    add("confirm", json!({}), "确认应用成功（解除 apply-guard）");
    add("revert_last_change", json!({}), "回滚最近一次写入");
    add(
        "restore",
        json!({ "mode": "string", "snapshot": "string" }),
        "按快照恢复（默认初始化基线）",
    );
    add("wifi_source_get", json!({}), "读取 Wi-Fi 信息源");
    add(
        "wifi_source_set",
        json!({
            "kind": "string",
            "gateway_endpoint": "string",
            "local_wifi_change_confirmed": "boolean",
            "custom": {},
        }),
        "设置 Wi-Fi 信息源",
    );
    add(
        "gateway_set",
        json!({ "lan_ifaces": {} }),
        "记录 Gateway 的 LAN 接口（不修改网络）",
    );
    add(
        "probe_connectivity",
        json!({ "target": "string" }),
        "只读连通性探测",
    );
    add("admission_list", json!({}), "准入列表");
    add(
        "admission_register",
        json!({ "device_id": "string", "mac": "string" }),
        "设备上报（AP 侧）",
    );
    add(
        "admission_approve",
        json!({ "device_id": "string" }),
        "批准设备",
    );
    add(
        "admission_reject",
        json!({ "device_id": "string" }),
        "拒绝设备",
    );
    add(
        "admission_revoke",
        json!({ "device_id": "string" }),
        "撤销批准",
    );
    add("backup_list", json!({}), "快照列表");
    add(
        "backup_verify",
        json!({ "path": "string" }),
        "校验快照完整性",
    );
    add("backup_create", json!({ "kind": "string" }), "手动创建快照");
    add("backup_prune", json!({}), "按保留策略清理快照");
    add("failsafe_get", json!({}), "读取故障恢复配置与状态");
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
        "设置故障恢复",
    );
    add("profile_get", json!({}), "读取网络档案");
    add("profile_publish", json!({ "profile": {} }), "下发网络档案");
    add(
        "logs_tail",
        json!({ "lines": "integer" }),
        "读取服务日志尾部",
    );
    add("version", json!({}), "版本信息");

    json!({ "wifisync": { "methods": methods } })
}

/// ubus 方法名（下划线风格，LuCI 侧习惯）→ 内部 RPC 方法名（点号风格）。
///
/// 用显式映射而不是无脑替换，避免 `plan_dry_run` 被误映射成 `plan.dry.run`。
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
        // 无歧义的方法直接透传
        other => other,
    };
    mapped.to_string()
}

/// 处理 `wifisync ubus <subcommand>`。
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
                .ok_or_else(|| "用法: wifisync ubus call <method>".to_string())?;
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
                    // rpcd 约定：失败时把原因写到 stderr 并以非零退出码结束
                    eprintln!("{}", error);
                    Ok(1)
                }
            }
        }
        _ => Err("用法: wifisync ubus [list|call <method>]".to_string()),
    }
}

fn read_stdin_json() -> Result<Value, String> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .map_err(|e| format!("读取 stdin 失败：{}", e))?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(trimmed).map_err(|e| format!("参数不是合法 JSON：{}", e))
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
            assert!(methods.get(name).is_some(), "缺少方法 {}", name);
        }
    }

    #[test]
    fn method_names_map_to_rpc_dotted_form() {
        assert_eq!(to_rpc_method("roles_get"), "roles.get");
        assert_eq!(to_rpc_method("plan_dry_run"), "plan.dry_run");
        assert_eq!(to_rpc_method("admission_approve"), "admission.approve");
        assert_eq!(to_rpc_method("probe_connectivity"), "probe.connectivity");
        // 无歧义方法原样透传
        assert_eq!(to_rpc_method("apply"), "apply");
        assert_eq!(to_rpc_method("status"), "status");
    }

    /// 方法表里的每个 ubus 方法都必须能映射到一个真实存在的内部方法。
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
                "ubus 方法 {} 映射到未知内部方法 {}",
                name,
                mapped
            );
        }
    }
}
