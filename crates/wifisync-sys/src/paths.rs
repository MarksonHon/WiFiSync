//! 目录与文件路径。
//!
//! 所有路径都基于 [`Paths::root`]，便于测试时重定向到临时目录
//! （环境变量 `WIFISYNC_ROOT`）。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self::new()
    }
}

impl Paths {
    /// 生产环境：`root = "/"`；若设置了 `WIFISYNC_ROOT` 则使用它（测试用）。
    pub fn new() -> Self {
        let root = std::env::var("WIFISYNC_ROOT").unwrap_or_else(|_| "/".to_string());
        Self {
            root: PathBuf::from(root),
        }
    }

    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `/etc/config`
    pub fn etc_config(&self) -> PathBuf {
        self.root.join("etc/config")
    }

    /// `/etc/config/<pkg>`
    pub fn uci_file(&self, pkg: &str) -> PathBuf {
        self.etc_config().join(pkg)
    }

    /// `/etc/wifisync`
    pub fn persistent_dir(&self) -> PathBuf {
        self.root.join("etc/wifisync")
    }

    /// `/etc/wifisync/backup`
    pub fn backup_dir(&self) -> PathBuf {
        self.persistent_dir().join("backup")
    }

    /// 不可变的初始化基线目录。
    pub fn initial_snapshot_dir(&self) -> PathBuf {
        self.backup_dir().join("initial")
    }

    /// `/etc/wifisync/secrets`（0600）
    pub fn secrets_dir(&self) -> PathBuf {
        self.persistent_dir().join("secrets")
    }

    /// `/etc/wifisync/state.json`（准入登记簿、档案版本号等）
    pub fn state_file(&self) -> PathBuf {
        self.persistent_dir().join("state.json")
    }

    /// `/etc/wifisync/device-id`
    pub fn device_id_file(&self) -> PathBuf {
        self.persistent_dir().join("device-id")
    }

    /// 运行时目录（tmpfs）：`/var/run/wifisync`
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("var/run/wifisync")
    }

    /// UNIX socket：`/var/run/wifisync/wifisync.sock`
    pub fn socket_file(&self) -> PathBuf {
        self.run_dir().join("wifisync.sock")
    }

    /// 「服务正在运行但未正常收尾」的脏标记。
    pub fn dirty_marker(&self) -> PathBuf {
        self.run_dir().join("dirty")
    }

    /// 服务日志（procd 也会收 stderr，这里额外留一份便于诊断）。
    pub fn log_file(&self) -> PathBuf {
        self.persistent_dir().join("wifisync.log")
    }

    pub fn ensure_dir(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        Ok(())
    }

    /// 需要纳入基线的 uci 配置文件（存在才备份）。
    pub fn baseline_uci_files(&self) -> Vec<&'static str> {
        vec!["network", "wireless", "dhcp", "firewall", "system"]
    }
}
