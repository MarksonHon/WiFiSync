//! The WifiSync executable.
//!
//! One binary with three modes:
//!
//! ```text
//! wifisync daemon                  # resident service (started by procd)
//! wifisync status|plan|apply|...   # command line
//! wifisync ubus list|call <m>      # rpcd exec plugin (used by LuCI)
//! ```

mod ctl;
mod daemon;
#[macro_use]
mod log;
mod probe;
mod rpc;
mod secrets;
mod signals;
mod ubus;

use daemon::{Daemon, DaemonOptions};
use serde_json::json;
use std::sync::Arc;
use wifisync_sys::Paths;

const USAGE: &str = "\
wifisync — home LAN (wired + wireless) networking tool for OpenWrt

usage:
  wifisync daemon [--no-restore-on-stop] [--json-log]
  wifisync status
  wifisync plan                      # dry-run: print the changes that would be applied
  wifisync apply                     # apply (only a pure AP role produces changes)
  wifisync confirm                   # confirm a successful apply (releases apply-guard)
  wifisync revert                    # revert the most recent write
  wifisync backup [list|create|verify|prune]
  wifisync restore [--full] [--snapshot initial|<path>]
  wifisync admit list|approve|reject|revoke [device_id]
  wifisync roles [controller ap gateway]
  wifisync probe <host[:port]>       # read-only connectivity probe
  wifisync ubus list|call <method>   # rpcd plugin mode
  wifisync version

notes:
  * the Gateway / Controller roles never modify any network configuration;
  * only a \"pure AP\" device (AP without Controller and Gateway) merges all ports into br-lan;
  * the initial network baseline is saved before the service starts and restored before it stops.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match dispatch(&args) {
        Ok(code) => code,
        Err(message) => {
            log::error(&message);
            eprintln!("{}", message);
            1
        }
    };
    std::process::exit(code);
}

fn dispatch(args: &[String]) -> Result<i32, String> {
    let paths = ctl::default_paths().map_err(|e| e.to_string())?;
    let command = args.first().map(|s| s.as_str()).unwrap_or("help");

    match command {
        "help" | "--help" | "-h" => {
            println!("{}", USAGE);
            Ok(0)
        }
        "version" | "--version" | "-V" => {
            ctl::print_json(&json!({
                "version": wifisync_core::VERSION,
                "protocol": 1,
            }));
            Ok(0)
        }
        "daemon" => run_daemon(&paths, &args[1..]),
        "status" => print(&ctl::status(&paths)?),
        "plan" => print(&ctl::plan(&paths)?),
        "apply" => print(&ctl::apply(&paths)?),
        "confirm" => print(&ctl::confirm(&paths)?),
        "revert" => print(&ctl::revert(&paths)?),
        "backup" => print(&ctl::backup(
            &paths,
            args.get(1).map(|s| s.as_str()).unwrap_or("create"),
        )?),
        "restore" => {
            let mut full = false;
            let mut snapshot: Option<String> = None;
            let mut index = 1;
            while index < args.len() {
                match args[index].as_str() {
                    "--full" => full = true,
                    "--snapshot" => {
                        index += 1;
                        snapshot = args.get(index).cloned();
                    }
                    other if !other.starts_with("--") => snapshot = Some(other.to_string()),
                    other => return Err(format!("unknown option `{}`", other)),
                }
                index += 1;
            }
            print(&ctl::restore(&paths, full, snapshot)?)
        }
        "admit" => print(&ctl::admit(
            &paths,
            args.get(1).map(|s| s.as_str()).unwrap_or("list"),
            args.get(2).map(|s| s.as_str()),
        )?),
        "roles" => print(&ctl::roles(&paths, args.get(1).map(|s| s.as_str()))?),
        "probe" => {
            let target = args
                .get(1)
                .ok_or_else(|| "usage: wifisync probe <host[:port]>".to_string())?;
            print(&ctl::probe(&paths, target)?)
        }
        "ubus" => ubus::run(&paths, &args[1..]),
        other => Err(format!("unknown command `{}`\n\n{}", other, USAGE)),
    }
}

fn print(value: &serde_json::Value) -> Result<i32, String> {
    ctl::print_json(value);
    Ok(0)
}

fn run_daemon(paths: &Paths, flags: &[String]) -> Result<i32, String> {
    let mut options = DaemonOptions::default();
    let mut foreground = false;
    for flag in flags {
        match flag.as_str() {
            "--no-restore-on-stop" => options.no_restore_on_stop = true,
            "--json-log" => {
                options.json_log = true;
                log::set_json_mode(true);
            }
            "--foreground" | "-f" => foreground = true,
            other => return Err(format!("unknown option `{}`", other)),
        }
    }
    let _ = foreground;

    signals::install();
    let daemon =
        Daemon::new(paths.clone(), options).map_err(|e| format!("startup failed: {}", e))?;
    // fail-closed: refuse to start without an initial baseline
    wifisync_sys::snapshot::require_baseline(paths)
        .map_err(|e| format!("{} (refusing to start without an initial baseline)", e))?;
    Arc::new(daemon)
        .start()
        .map_err(|e| format!("run failed: {}", e))?;
    Ok(0)
}
