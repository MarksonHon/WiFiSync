//! Restore executor (requirement 11).
//!
//! It only executes the [`RestorePlan`] computed by `wifisync-core`; it makes no policy
//! decisions of its own.

use crate::error::{SysError, SysResult};
use crate::netifd;
use crate::paths::Paths;
use crate::snapshot::SnapshotStore;
use crate::uci::Uci;
use std::path::{Path, PathBuf};
use wifisync_core::backup::BackupManifest;
use wifisync_core::backup::{
    build_restore_plan, ManagedEntry, RestoreAction, RestoreMode, RestorePlan, VerifyReport,
};
use wifisync_core::uci_file;
use wifisync_core::Message;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    pub applied: Vec<String>,
    pub failed: Vec<String>,
    pub blocked: Option<String>,
    pub reloaded_network: bool,
    pub reloaded_wifi: bool,
}

impl RestoreReport {
    pub fn success(&self) -> bool {
        self.failed.is_empty() && self.blocked.is_none()
    }

    pub fn summary(&self) -> String {
        if let Some(reason) = &self.blocked {
            return format!("restore blocked: {}", reason);
        }
        format!(
            "restored {} item(s){}",
            self.applied.len(),
            if self.failed.is_empty() {
                String::new()
            } else {
                format!(", {} failed", self.failed.len())
            }
        )
    }

    /// UI message describing the restore result (translated by the front end).
    pub fn summary_message(&self) -> Message {
        if let Some(reason) = &self.blocked {
            return Message::new("restore.blocked").param("reason", reason.clone());
        }
        Message::new("restore.done")
            .param("restored", self.applied.len().to_string())
            .param("failed", self.failed.len().to_string())
    }
}

pub struct Restorer {
    paths: Paths,
    uci: Uci,
}

impl Restorer {
    pub fn new(paths: Paths) -> Self {
        let uci = Uci::new(paths.clone());
        Self { paths, uci }
    }

    pub fn snapshot_store(&self) -> SnapshotStore {
        SnapshotStore::new(self.paths.clone())
    }

    /// Build a restore plan (including integrity verification).
    pub fn plan(&self, mode: RestoreMode, snapshot_dir: &Path) -> SysResult<RestorePlan> {
        let store = self.snapshot_store();
        let manifest = store.manifest(snapshot_dir)?;
        let verification = store.verify(snapshot_dir)?;
        let entries = self.managed_entries(&manifest, snapshot_dir)?;
        Ok(build_restore_plan(
            mode,
            &manifest,
            &snapshot_dir.to_string_lossy(),
            &verification,
            &entries,
        ))
    }

    /// Collect the "original value / current value" comparison for managed keys.
    pub fn managed_entries(
        &self,
        manifest: &BackupManifest,
        snapshot_dir: &Path,
    ) -> SysResult<Vec<ManagedEntry>> {
        let mut entries = Vec::new();
        for key in &manifest.managed_keys {
            let Some((file, section, option)) = uci_file::split_key(key) else {
                continue;
            };
            let Some(option) = option else {
                continue;
            };

            // Original value: from the snapshot's config file text
            let original = std::fs::read_to_string(snapshot_dir.join("config").join(&file))
                .ok()
                .and_then(|text| uci_file::get(&text, &section, &option));

            // Current value: prefer uci, fall back to reading the file directly
            let current = match self.uci.get(&format!("{}.{}.{}", file, section, option))? {
                Some(value) => Some(value),
                None => std::fs::read_to_string(self.paths.uci_file(&file))
                    .ok()
                    .and_then(|text| uci_file::get(&text, &section, &option)),
            };

            entries.push(ManagedEntry {
                key: key.clone(),
                original,
                current,
            });
        }
        Ok(entries)
    }

    /// Execute the restore plan.
    pub fn execute(&self, plan: &RestorePlan, snapshot_dir: &Path) -> SysResult<RestoreReport> {
        let mut report = RestoreReport {
            applied: Vec::new(),
            failed: Vec::new(),
            blocked: plan.blocked.clone(),
            reloaded_network: false,
            reloaded_wifi: false,
        };
        if plan.is_blocked() {
            return Ok(report);
        }

        let mut touched_files: Vec<String> = Vec::new();

        for action in &plan.actions {
            match action {
                RestoreAction::SetOption { key, value } => {
                    let (file, section, option) = split3(key)?;
                    let expr = format!("{}.{}.{}={}", file, section, option, value);
                    match crate::exec::run_ok("uci", &["set", &expr]) {
                        Ok(_) => {
                            report.applied.push(expr);
                            push_unique(&mut touched_files, file);
                        }
                        Err(e) => report.failed.push(format!("{}: {}", key, e)),
                    }
                }
                RestoreAction::DeleteOption { key } => {
                    let target = key.clone();
                    match crate::exec::run("uci", &["-q", "delete", &target]) {
                        Ok(_) => {
                            if let Ok((file, _, _)) = split3(key) {
                                push_unique(&mut touched_files, file);
                            }
                            report.applied.push(format!("delete {}", key));
                        }
                        Err(e) => report.failed.push(format!("delete {}: {}", key, e)),
                    }
                }
                RestoreAction::ReplaceFile { file, .. } => {
                    let name = PathBuf::from(file)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| file.clone());
                    let source = snapshot_dir.join(file);
                    let target = self.paths.uci_file(&name);
                    match std::fs::copy(&source, &target) {
                        Ok(_) => {
                            report.applied.push(format!("restore /etc/config/{}", name));
                            push_unique(&mut touched_files, name);
                        }
                        Err(e) => report.failed.push(format!("restore {}: {}", name, e)),
                    }
                }
                RestoreAction::ReloadNetwork => report.reloaded_network = true,
                RestoreAction::ReloadWifi => report.reloaded_wifi = true,
            }
        }

        // commit: also commit once after file-level replacement to keep uci state consistent
        for file in &touched_files {
            if let Err(e) = crate::exec::run_ok("uci", &["commit", file]) {
                report.failed.push(format!("commit {}: {}", file, e));
            }
        }

        if report.reloaded_network || report.reloaded_wifi {
            if report.reloaded_wifi {
                if let Err(e) = netifd::reload_wifi() {
                    report.failed.push(format!("reload wifi: {}", e));
                }
            } else if let Err(e) = netifd::reload_network() {
                report.failed.push(format!("reload network: {}", e));
            }
        }

        Ok(report)
    }

    /// Run a read-only verification after the restore completes.
    pub fn verify_after_restore(&self, snapshot_dir: &Path) -> SysResult<VerifyReport> {
        self.snapshot_store().verify(snapshot_dir)
    }
}

fn split3(key: &str) -> SysResult<(String, String, String)> {
    match uci_file::split_key(key) {
        Some((file, section, Some(option))) => Ok((file, section, option)),
        _ => Err(SysError::Parse {
            what: "managed key".to_string(),
            message: format!("`{}` is not in file.section.option form", key),
        }),
    }
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wifisync_core::backup::{SnapshotKind, VerifyReport};
    use wifisync_core::role::Roles;

    #[test]
    fn blocked_plan_reports_reason() {
        let dir = std::env::temp_dir().join("wifisync-restore-blocked");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let restorer = Restorer::new(Paths::with_root(&dir));
        let plan = RestorePlan {
            mode: RestoreMode::ManagedOnly,
            actions: vec![],
            skipped: vec![],
            blocked: Some("verification failed".into()),
        };
        let report = restorer.execute(&plan, &dir).unwrap();
        assert!(!report.success());
        assert!(report.summary().contains("verification failed"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn managed_entries_read_original_from_snapshot() {
        let dir = std::env::temp_dir().join("wifisync-restore-entries");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("snap/config")).unwrap();
        std::fs::create_dir_all(dir.join("etc/config")).unwrap();
        std::fs::write(
            dir.join("snap/config/network"),
            "config interface 'lan'\n\toption ipaddr '192.168.1.1'\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("etc/config/network"),
            "config interface 'lan'\n\toption ipaddr '10.0.0.1'\n",
        )
        .unwrap();

        let restorer = Restorer::new(Paths::with_root(&dir));
        let mut manifest = BackupManifest::new(SnapshotKind::Initial, 1, "dev", Roles::default());
        manifest.managed_keys = vec!["network.lan.ipaddr".into()];
        let entries = restorer
            .managed_entries(&manifest, &dir.join("snap"))
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].original.as_deref(), Some("192.168.1.1"));
        assert_eq!(entries[0].current.as_deref(), Some("10.0.0.1"));

        let plan = build_restore_plan(
            RestoreMode::ManagedOnly,
            &manifest,
            &dir.join("snap").to_string_lossy(),
            &VerifyReport::default(),
            &entries,
        );
        assert_eq!(plan.actions.len(), 2); // set + reload

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn split3_rejects_short_keys() {
        assert!(split3("network.lan").is_err());
        assert!(split3("network.lan.ipaddr").is_ok());
    }
}
