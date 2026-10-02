//! Network parameter snapshots (requirement 11).
//!
//! Directory layout:
//!
//! ```text
//! /etc/wifisync/backup/
//! ├─ initial/                  # immutable initial baseline (authoritative restore source)
//! │  ├─ manifest.json
//! │  ├─ config/{network,wireless,dhcp,firewall,system}
//! │  └─ state.txt
//! ├─ pre-start-<ts>/
//! └─ pre-change-<ts>/
//! ```
//!
//! Everything is plain text, which keeps diffs easy and size under control.

use crate::error::{SysError, SysResult};
use crate::iwinfo;
use crate::paths::Paths;
use crate::state::{random_hex, write_atomic};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use wifisync_core::backup::{
    verify, BackupManifest, FileDigest, RetentionPolicy, SnapshotKind, SnapshotMeta, VerifyReport,
};
use wifisync_core::config::WifisyncConfig;
use wifisync_core::role::Roles;

/// Name of the config subdirectory inside a snapshot directory.
const CONFIG_SUBDIR: &str = "config";
const STATE_FILE_NAME: &str = "state.txt";
const MANIFEST_NAME: &str = "manifest.json";

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub struct SnapshotStore {
    paths: Paths,
}

impl SnapshotStore {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Whether the initial baseline exists and is readable.
    pub fn initial_exists(&self) -> bool {
        self.paths
            .initial_snapshot_dir()
            .join(MANIFEST_NAME)
            .exists()
    }

    /// The first thing on service start: **establish (or verify) the initial baseline**.
    ///
    /// Returns `(manifest, created)`; `created = false` means an existing baseline was reused.
    pub fn ensure_initial(&self, cfg: &WifisyncConfig) -> SysResult<(BackupManifest, bool)> {
        if self.initial_exists() {
            let dir = self.paths.initial_snapshot_dir();
            let manifest = self.manifest(&dir)?;
            return Ok((manifest, false));
        }
        let manifest = self.create(SnapshotKind::Initial, cfg, &[])?;
        Ok((manifest, true))
    }

    pub fn dir_for(&self, kind: SnapshotKind, created_at: i64) -> PathBuf {
        match kind {
            SnapshotKind::Initial => self.paths.initial_snapshot_dir(),
            _ => self
                .paths
                .backup_dir()
                .join(format!("{}-{}", kind.dir_prefix(), created_at)),
        }
    }

    /// Create a snapshot.
    pub fn create(
        &self,
        kind: SnapshotKind,
        cfg: &WifisyncConfig,
        managed_keys: &[String],
    ) -> SysResult<BackupManifest> {
        let created_at = now();
        let dir = self.dir_for(kind, created_at);
        if kind.is_immutable() && dir.exists() {
            // The baseline is immutable: reuse it as-is if it already exists
            return self.manifest(&dir);
        }
        let config_dir = dir.join(CONFIG_SUBDIR);
        self.paths.ensure_dir(&config_dir)?;

        let mut manifest = BackupManifest::new(kind, created_at, &cfg.device_id, cfg.roles);
        manifest.managed_keys = managed_keys.to_vec();
        manifest.notes.push(format!(
            "created by wifisync {} with roles [{}]",
            wifisync_core::VERSION,
            cfg.roles.to_uci_value()
        ));

        // 1. uci config files (backed up only if present)
        for pkg in self.paths.baseline_uci_files() {
            let source = self.paths.uci_file(pkg);
            if !source.exists() {
                continue;
            }
            let target = config_dir.join(pkg);
            std::fs::copy(&source, &target)?;
            manifest
                .files
                .push(digest_of(&target, &format!("{}/{}", CONFIG_SUBDIR, pkg))?);
        }

        // 2. read-only state dump (ip / iwinfo / bridge members)
        let dump = iwinfo::capture_state(&self.paths);
        let state_path = dir.join(STATE_FILE_NAME);
        write_atomic(&state_path, dump.as_bytes())?;
        manifest
            .files
            .push(digest_of(&state_path, STATE_FILE_NAME)?);

        // 3. the manifest itself
        let text = serde_json::to_string_pretty(&manifest)?;
        write_atomic(&dir.join(MANIFEST_NAME), text.as_bytes())?;
        Ok(manifest)
    }

    pub fn manifest(&self, dir: &Path) -> SysResult<BackupManifest> {
        let text = std::fs::read_to_string(dir.join(MANIFEST_NAME))?;
        Ok(serde_json::from_str(&text)?)
    }

    /// List metadata for all snapshots.
    pub fn list(&self) -> SysResult<Vec<SnapshotMeta>> {
        let root = self.paths.backup_dir();
        let mut items = Vec::new();
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(items);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Ok(manifest) = self.manifest(&path) else {
                continue;
            };
            items.push(SnapshotMeta {
                kind: manifest.kind,
                created_at: manifest.created_at,
                path: path.to_string_lossy().to_string(),
                bytes: dir_size(&path),
            });
        }
        items.sort_by_key(|item| item.created_at);
        Ok(items)
    }

    /// Verify snapshot integrity (recomputing sha256).
    pub fn verify(&self, dir: &Path) -> SysResult<VerifyReport> {
        let manifest = self.manifest(dir)?;
        let mut computed = Vec::new();
        for file in &manifest.files {
            let path = dir.join(&file.path);
            if !path.exists() {
                continue;
            }
            computed.push(digest_of(&path, &file.path)?);
        }
        Ok(verify(&manifest, &computed))
    }

    /// Prune according to the retention policy (`initial` is never pruned).
    pub fn prune(&self, policy: &RetentionPolicy) -> SysResult<Vec<String>> {
        let snapshots = self.list()?;
        let doomed = policy.plan_prune(&snapshots);
        for path in &doomed {
            let _ = std::fs::remove_dir_all(path);
        }
        Ok(doomed)
    }

    /// Record the "managed keys touched by this round's write plan" into the latest
    /// snapshot's manifest, so restore can reproduce them precisely.
    pub fn annotate_managed_keys(&self, dir: &Path, keys: &[String]) -> SysResult<()> {
        let mut manifest = self.manifest(dir)?;
        for key in keys {
            if !manifest.managed_keys.contains(key) {
                manifest.managed_keys.push(key.clone());
            }
        }
        manifest.managed_keys.sort();
        manifest.managed_keys.dedup();
        let text = serde_json::to_string_pretty(&manifest)?;
        write_atomic(&dir.join(MANIFEST_NAME), text.as_bytes())
    }

    /// Write a "work marker" recording the latest pre-change snapshot path, used by
    /// apply-guard for rollback.
    pub fn write_last_change_marker(&self, dir: &Path) -> SysResult<()> {
        self.paths.ensure_dir(&self.paths.run_dir())?;
        write_atomic(
            &self.paths.run_dir().join("last-change"),
            dir.to_string_lossy().as_bytes(),
        )
    }

    pub fn last_change_dir(&self) -> Option<PathBuf> {
        std::fs::read_to_string(self.paths.run_dir().join("last-change"))
            .ok()
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| p.exists())
    }
}

/// Compute a file digest.
pub fn digest_of(path: &Path, rel_path: &str) -> SysResult<FileDigest> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(FileDigest {
        path: rel_path.to_string(),
        sha256: hex(&hasher.finalize()),
        size: bytes.len() as u64,
    })
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

/// On first start, generate a device-id if missing (the snapshot manifest needs it).
pub fn ensure_device_id(paths: &Paths) -> SysResult<String> {
    let path = paths.device_id_file();
    if let Ok(text) = std::fs::read_to_string(&path) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let id = random_hex(16)?;
    paths.ensure_dir(&paths.persistent_dir())?;
    write_atomic(&path, id.as_bytes())?;
    Ok(id)
}

/// Convenience helper that writes a role set into a manifest.
pub fn with_roles(mut manifest: BackupManifest, roles: Roles) -> BackupManifest {
    manifest.roles = roles;
    manifest
}

/// Assert that the baseline exists, otherwise return a fail-closed error.
pub fn require_baseline(paths: &Paths) -> SysResult<()> {
    let dir = paths.initial_snapshot_dir();
    if dir.join(MANIFEST_NAME).exists() {
        Ok(())
    } else {
        Err(SysError::BaseLineMissing(dir.to_string_lossy().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wifisync_core::backup::SnapshotKind;

    fn setup(tag: &str) -> (Paths, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("wifisync-snap-{}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("etc/config")).unwrap();
        std::fs::write(
            dir.join("etc/config/network"),
            "config interface 'lan'\n\toption ipaddr '192.168.1.1'\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("etc/config/wireless"),
            "config wifi-device 'radio0'\n",
        )
        .unwrap();
        (Paths::with_root(&dir), dir)
    }

    #[test]
    fn initial_snapshot_is_created_once_and_reused() {
        let (paths, dir) = setup("initial");
        let store = SnapshotStore::new(paths);
        let cfg = WifisyncConfig::default();

        let (first, created) = store.ensure_initial(&cfg).unwrap();
        assert!(created);
        assert_eq!(first.kind, SnapshotKind::Initial);
        assert!(first.files.iter().any(|f| f.path == "config/network"));

        let (second, created_again) = store.ensure_initial(&cfg).unwrap();
        assert!(
            !created_again,
            "the baseline is immutable and must not be recreated"
        );
        assert_eq!(second.created_at, first.created_at);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verification_detects_tampering() {
        let (paths, dir) = setup("verify");
        let store = SnapshotStore::new(paths);
        let cfg = WifisyncConfig::default();
        store.ensure_initial(&cfg).unwrap();

        let snapshot_dir = store.paths().initial_snapshot_dir();
        assert!(store.verify(&snapshot_dir).unwrap().is_ok());

        std::fs::write(snapshot_dir.join("config/network"), "tampered").unwrap();
        let report = store.verify(&snapshot_dir).unwrap();
        assert!(!report.is_ok());
        assert!(report.changed.contains(&"config/network".to_string()));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn list_and_prune_keep_initial() {
        let (paths, dir) = setup("prune");
        let store = SnapshotStore::new(paths);
        let cfg = WifisyncConfig::default();
        store.ensure_initial(&cfg).unwrap();
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(1100));
            store.create(SnapshotKind::PreStart, &cfg, &[]).unwrap();
        }
        let items = store.list().unwrap();
        assert!(items.iter().any(|i| i.kind == SnapshotKind::Initial));

        let doomed = store
            .prune(&RetentionPolicy {
                keep_rolling: 1,
                max_bytes: None,
            })
            .unwrap();
        assert!(!doomed.iter().any(|p| p.ends_with("initial")));
        assert!(store.initial_exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn managed_keys_can_be_annotated() {
        let (paths, dir) = setup("annotate");
        let store = SnapshotStore::new(paths);
        let cfg = WifisyncConfig::default();
        store.ensure_initial(&cfg).unwrap();
        let snapshot_dir = store.paths().initial_snapshot_dir();
        store
            .annotate_managed_keys(
                &snapshot_dir,
                &[
                    "network.br-lan.type".to_string(),
                    "network.br-lan.ports".to_string(),
                ],
            )
            .unwrap();
        let manifest = store.manifest(&snapshot_dir).unwrap();
        assert_eq!(manifest.managed_keys.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn baseline_requirement_is_fail_closed() {
        let dir = std::env::temp_dir().join("wifisync-snap-none");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths::with_root(&dir);
        assert!(require_baseline(&paths).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
