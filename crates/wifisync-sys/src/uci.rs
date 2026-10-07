//! uci read/write. The **only** entry point allowed to write `/etc/config`, and it only
//! accepts write plans produced by `wifisync-core`.

use crate::error::{SysError, SysResult};
use crate::exec;
use crate::paths::Paths;
use wifisync_core::config::{WifisyncConfig, UCI_FILE};
use wifisync_core::plan::{UciOp, UciOpKind};
use wifisync_core::uci_file;

pub struct Uci {
    paths: Paths,
}

impl Uci {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    /// Is uci available on this system (the host unit-test environment often lacks it)?
    pub fn available(&self) -> bool {
        exec::has("uci")
    }

    /// `uci -q get <key>`; returns `None` when the key is absent or uci is missing.
    pub fn get(&self, key: &str) -> SysResult<Option<String>> {
        let out = match exec::run("uci", &["-q", "get", key]) {
            Ok(out) => out,
            // Without uci on a host/minimal system this degrades to "not readable"; the
            // caller falls back to reading the file directly
            Err(SysError::MissingProgram(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        if !out.success() {
            return Ok(None);
        }
        Ok(Some(out.stdout_trimmed().to_string()))
    }

    /// Read a config file directly as text (for backup and parsing; does not call uci).
    pub fn read_config_file(&self, pkg: &str) -> SysResult<Option<String>> {
        let path = self.paths.uci_file(pkg);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(std::fs::read_to_string(path)?))
    }

    /// `uci export <pkg>` (normalized output).
    pub fn export(&self, pkg: &str) -> SysResult<String> {
        let out = exec::run("uci", &["-q", "export", pkg])?;
        Ok(out.stdout)
    }

    /// Read WifiSync's own config. Prefers the file (works without uci, aiding offline diagnosis).
    pub fn load_config(&self) -> SysResult<WifisyncConfig> {
        match self.read_config_file(UCI_FILE)? {
            Some(text) => Ok(WifisyncConfig::from_sections(&uci_file::parse(&text))),
            None => Ok(WifisyncConfig::default()),
        }
    }

    /// Execute a write plan and commit. Returns the uci commands actually run (for audit
    /// and dry-run comparison).
    pub fn apply_ops(&self, ops: &[UciOp]) -> SysResult<Vec<String>> {
        let mut executed = Vec::new();
        let mut touched: Vec<String> = Vec::new();

        for op in ops {
            let (subcommand, expr) = match op.kind {
                // `uci set file.section=type` creates a section (option is None)
                UciOpKind::Set => (
                    "set",
                    format!("{}={}", op.key(), op.value.clone().unwrap_or_default()),
                ),
                UciOpKind::AddList => (
                    "add_list",
                    format!("{}={}", op.key(), op.value.clone().unwrap_or_default()),
                ),
                // The caller substituted the real secret: write it, but keep it out of the audit
                // trail that is returned to the CLI and to the UI.
                UciOpKind::SetSecret => (
                    "set",
                    format!("{}={}", op.key(), op.value.clone().unwrap_or_default()),
                ),
                UciOpKind::Delete => ("delete", op.key()),
            };

            exec::run_ok("uci", &[subcommand, &expr])?;
            if op.kind == UciOpKind::SetSecret {
                executed.push(format!("uci {} {}=<secret>", subcommand, op.key()));
            } else {
                executed.push(format!("uci {} {}", subcommand, expr));
            }
            if !touched.contains(&op.file) {
                touched.push(op.file.clone());
            }
        }

        if !executed.is_empty() {
            for file in &touched {
                exec::run_ok("uci", &["commit", file])?;
                executed.push(format!("uci commit {}", file));
            }
        }
        Ok(executed)
    }

    /// Delete a section created by wifisync (used during restore).
    pub fn delete_section(&self, file: &str, section: &str) -> SysResult<()> {
        let out = exec::run("uci", &["-q", "delete", &format!("{}.{}", file, section)])?;
        if !out.success() {
            return Err(SysError::Command {
                program: "uci".to_string(),
                message: format!(
                    "deleting {}.{} failed: {}",
                    file,
                    section,
                    out.stderr.trim()
                ),
            });
        }
        Ok(())
    }
}
