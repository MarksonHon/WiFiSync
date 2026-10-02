//! netifd / ubus calls.
//!
//! **The caller must already have a non-empty write plan**
//! (see `wifisync_core::plan::WritePlan::is_empty`); otherwise reloading the network is a
//! disturbance by itself — Gateway / Controller roles must never reach this point.

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
    // Fallback path: OpenWrt's init script
    if exec::has("/etc/init.d/network") {
        exec::run_ok("/etc/init.d/network", &["reload"])?;
        return Ok(());
    }
    Err(SysError::MissingProgram(
        "ubus or /etc/init.d/network".into(),
    ))
}

/// `wifi reload` (reload network first, then wireless, matching OpenWrt's usual order)
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

/// Restart the network (used for failure recovery / restoring the baseline).
pub fn restart_network() -> SysResult<()> {
    if exec::has("/etc/init.d/network") {
        return exec::run_ok("/etc/init.d/network", &["restart"]).map(|_| ());
    }
    Err(SysError::MissingProgram("/etc/init.d/network".into()))
}

/// Reboot the device (when failsafe is configured as reboot).
pub fn reboot() -> SysResult<()> {
    exec::run_ok("reboot", &[]).map(|_| ())
}

/// Call an arbitrary ubus object (for read-only diagnostics).
pub fn call_ubus(object: &str, method: &str, args: &[&str]) -> SysResult<String> {
    let mut argv = vec!["call", object, method];
    argv.extend_from_slice(args);
    let out = exec::run("ubus", &argv)?;
    Ok(out.stdout)
}
