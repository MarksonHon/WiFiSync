//! Directories and file paths.
//!
//! All paths are rooted at [`Paths::root`], so tests can redirect them to a temp directory
//! (via the `WIFISYNC_ROOT` environment variable).

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
    /// Production: `root = "/"`; if `WIFISYNC_ROOT` is set it is used instead (for tests).
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

    /// Immutable initial baseline directory.
    pub fn initial_snapshot_dir(&self) -> PathBuf {
        self.backup_dir().join("initial")
    }

    /// `/etc/wifisync/secrets` (0600)
    pub fn secrets_dir(&self) -> PathBuf {
        self.persistent_dir().join("secrets")
    }

    /// `/etc/wifisync/state.json` (admission registry, profile version, etc.)
    pub fn state_file(&self) -> PathBuf {
        self.persistent_dir().join("state.json")
    }

    /// `/etc/wifisync/device-id`
    pub fn device_id_file(&self) -> PathBuf {
        self.persistent_dir().join("device-id")
    }

    /// Runtime directory (tmpfs): `/var/run/wifisync`
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("var/run/wifisync")
    }

    /// UNIX socket: `/var/run/wifisync/wifisync.sock`
    pub fn socket_file(&self) -> PathBuf {
        self.run_dir().join("wifisync.sock")
    }

    /// Dirty marker for "service is running but did not shut down cleanly".
    pub fn dirty_marker(&self) -> PathBuf {
        self.run_dir().join("dirty")
    }

    /// Service log (procd also captures stderr; this is an extra copy for diagnostics).
    pub fn log_file(&self) -> PathBuf {
        self.persistent_dir().join("wifisync.log")
    }

    pub fn ensure_dir(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        Ok(())
    }

    /// uci config files to include in the baseline (backed up only if present).
    pub fn baseline_uci_files(&self) -> Vec<&'static str> {
        vec!["network", "wireless", "dhcp", "firewall", "system"]
    }
}
