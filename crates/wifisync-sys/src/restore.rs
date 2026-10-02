//! 恢复执行器（需求 11）。
//!
//! 只执行 `wifisync-core` 计算出来的 [`RestorePlan`]，本身不做任何策略判断。

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
            return format!("恢复被阻止：{}", reason);
        }
        format!(
            "已恢复 {} 项{}",
            self.applied.len(),
            if self.failed.is_empty() {
                String::new()
            } else {
                format!("，失败 {} 项", self.failed.len())
            }
        )
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

    /// 生成恢复计划（含完整性校验）。
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

    /// 收集「受管键的原值 / 当前值」对比。
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

            // 原值：来自快照里的配置文件文本
            let original = std::fs::read_to_string(snapshot_dir.join("config").join(&file))
                .ok()
                .and_then(|text| uci_file::get(&text, &section, &option));

            // 当前值：优先 uci，退化为直接读文件
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

    /// 执行恢复计划。
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

        // commit：文件级替换后也 commit 一次，保持 uci 状态一致
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

    /// 恢复完成后做一次只读校验。
    pub fn verify_after_restore(&self, snapshot_dir: &Path) -> SysResult<VerifyReport> {
        self.snapshot_store().verify(snapshot_dir)
    }
}

fn split3(key: &str) -> SysResult<(String, String, String)> {
    match uci_file::split_key(key) {
        Some((file, section, Some(option))) => Ok((file, section, option)),
        _ => Err(SysError::Parse {
            what: "managed key".to_string(),
            message: format!("`{}` 不是 file.section.option 形式", key),
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
            blocked: Some("校验失败".into()),
        };
        let report = restorer.execute(&plan, &dir).unwrap();
        assert!(!report.success());
        assert!(report.summary().contains("校验失败"));
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
