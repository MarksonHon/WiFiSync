//! netifd / ubus 调用。
//!
//! **调用方必须先有非空的写入计划**（见 `wifisync_core::plan::WritePlan::is_empty`），
//! 否则重载网络本身就会造成一次扰动 —— Gateway / Controller 角色永远不该走到这里。

use crate::error::{SysError, SysResult};
use crate::exec;

/// `ubus call network reload`
pub fn reload_network() -> SysResult<()> {
    if exec::has("ubus") {
        let out = exec::run("ubus", &["call", "network", "reload"])?;
        if out.success() {
            return Ok(());
        }
    }
    // 退化路径：OpenWrt 的 init 脚本
    if exec::has("/etc/init.d/network") {
        exec::run_ok("/etc/init.d/network", &["reload"])?;
        return Ok(());
    }
    Err(SysError::MissingProgram(
        "ubus 或 /etc/init.d/network".into(),
    ))
}

/// `wifi reload`（先重载网络再重载无线，与 OpenWrt 惯用顺序一致）
pub fn reload_wifi() -> SysResult<()> {
    reload_network()?;
    if exec::has("/sbin/wifi") {
        exec::run_ok("/sbin/wifi", &["reload"])?;
        return Ok(());
    }
    if exec::has("wifi") {
        exec::run_ok("wifi", &["reload"])?;
        return Ok(());
    }
    Ok(())
}

/// 重启网络（故障恢复 / 还原基线时使用）。
pub fn restart_network() -> SysResult<()> {
    if exec::has("/etc/init.d/network") {
        return exec::run_ok("/etc/init.d/network", &["restart"]).map(|_| ());
    }
    Err(SysError::MissingProgram("/etc/init.d/network".into()))
}

/// 重启设备（failsafe 配置为 reboot 时）。
pub fn reboot() -> SysResult<()> {
    exec::run_ok("reboot", &[]).map(|_| ())
}

/// 调用任意 ubus 对象（只读诊断用）。
pub fn call_ubus(object: &str, method: &str, args: &[&str]) -> SysResult<String> {
    let mut argv = vec!["call", object, method];
    argv.extend_from_slice(args);
    let out = exec::run("ubus", &argv)?;
    Ok(out.stdout)
}
