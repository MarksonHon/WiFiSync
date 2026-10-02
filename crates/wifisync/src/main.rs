//! WifiSync 可执行程序。
//!
//! 一个二进制三种模式：
//!
//! ```text
//! wifisync daemon                  # 常驻服务（procd 拉起）
//! wifisync status|plan|apply|...   # 命令行
//! wifisync ubus list|call <m>      # rpcd exec 插件（LuCI 用）
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
wifisync — OpenWrt 家庭局域网组网工具

用法:
  wifisync daemon [--no-restore-on-stop] [--json-log]
  wifisync status
  wifisync plan                      # dry-run：打印将要执行的改动
  wifisync apply                     # 应用（仅纯 AP 角色会产生改动）
  wifisync confirm                   # 确认应用成功（解除 apply-guard）
  wifisync revert                    # 回滚最近一次写入
  wifisync backup [list|create|verify|prune]
  wifisync restore [--full] [--snapshot initial|<path>]
  wifisync admit list|approve|reject|revoke [device_id]
  wifisync roles [controller ap gateway]
  wifisync probe <host[:port]>       # 只读连通性探测
  wifisync ubus list|call <method>   # rpcd 插件模式
  wifisync version

说明:
  * Gateway / Controller 角色不会修改任何网络配置；
  * 只有「纯 AP」设备（有 AP 且无 Controller、无 Gateway）才会把所有网口并入 br-lan；
  * 服务启动前会保存初始化网络基线，停止前会恢复原有网络。
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
                    other => return Err(format!("未知选项 `{}`", other)),
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
                .ok_or_else(|| "用法: wifisync probe <host[:port]>".to_string())?;
            print(&ctl::probe(&paths, target)?)
        }
        "ubus" => ubus::run(&paths, &args[1..]),
        other => Err(format!("未知命令 `{}`\n\n{}", other, USAGE)),
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
            other => return Err(format!("未知选项 `{}`", other)),
        }
    }
    let _ = foreground;

    signals::install();
    let daemon = Daemon::new(paths.clone(), options).map_err(|e| format!("启动失败：{}", e))?;
    // fail-closed：没有初始化基线就拒绝启动
    wifisync_sys::snapshot::require_baseline(paths)
        .map_err(|e| format!("{}（拒绝在缺少初始化基线的情况下启动）", e))?;
    Arc::new(daemon)
        .start()
        .map_err(|e| format!("运行失败：{}", e))?;
    Ok(0)
}
