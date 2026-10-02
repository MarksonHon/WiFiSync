//! **Models and decisions** for backup / snapshot / restore (requirement 11).
//!
//! Discipline: **back up the initial network at startup, restore the original network at stop.**
//!
//! Only pure computation happens here:
//! * [`BackupManifest`]: snapshot manifest (per-file sha256 plus `managed_keys`)
//! * [`RetentionPolicy`]: retention policy (`initial` is never pruned)
//! * [`build_restore_plan`]: restore plan (defaults to `managed_only`, touching only keys wifisync
//!   changed)

use crate::message::Message;
use crate::role::Roles;
use serde::{Deserialize, Serialize};

/// Snapshot format version.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    /// Immutable initial baseline (the authoritative restore source).
    Initial,
    /// Rolling snapshot taken before every service start.
    PreStart,
    /// Rolling snapshot taken before every write.
    PreChange,
}

impl SnapshotKind {
    pub fn dir_prefix(&self) -> &'static str {
        match self {
            SnapshotKind::Initial => "initial",
            SnapshotKind::PreStart => "pre-start",
            SnapshotKind::PreChange => "pre-change",
        }
    }

    pub fn is_immutable(&self) -> bool {
        matches!(self, SnapshotKind::Initial)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDigest {
    /// Path relative to the snapshot directory, e.g. `config/network`.
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// Snapshot manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub format_version: u32,
    pub kind: SnapshotKind,
    pub created_at: crate::Timestamp,
    pub device_id: String,
    pub wifisync_version: String,
    /// Role set at snapshot time (used for post-restore consistency hints).
    pub roles: Roles,
    pub files: Vec<FileDigest>,
    /// uci key paths changed by wifisync (in `network.lan.ipaddr` form).
    pub managed_keys: Vec<String>,
    pub notes: Vec<String>,
}

impl BackupManifest {
    pub fn new(
        kind: SnapshotKind,
        created_at: crate::Timestamp,
        device_id: impl Into<String>,
        roles: Roles,
    ) -> Self {
        Self {
            format_version: SNAPSHOT_FORMAT_VERSION,
            kind,
            created_at,
            device_id: device_id.into(),
            wifisync_version: crate::VERSION.to_string(),
            roles,
            files: Vec::new(),
            managed_keys: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn file(&self, path: &str) -> Option<&FileDigest> {
        self.files.iter().find(|f| f.path == path)
    }

    pub fn is_initial(&self) -> bool {
        self.kind.is_immutable()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyReport {
    pub missing: Vec<String>,
    pub changed: Vec<String>,
    pub extra: Vec<String>,
}

impl VerifyReport {
    pub fn is_ok(&self) -> bool {
        self.missing.is_empty() && self.changed.is_empty()
    }

    pub fn summary(&self) -> String {
        if self.is_ok() {
            return "backup integrity check passed".to_string();
        }
        format!(
            "backup check failed: {} missing, {} changed, {} extra",
            self.missing.len(),
            self.changed.len(),
            self.extra.len()
        )
    }

    /// UI message describing the verification result.
    pub fn summary_message(&self) -> Message {
        if self.is_ok() {
            return Message::new("backup.verify.ok");
        }
        Message::new("backup.verify.failed")
            .param("missing", self.missing.len().to_string())
            .param("changed", self.changed.len().to_string())
            .param("extra", self.extra.len().to_string())
    }
}

/// Compare the manifest against the actually computed digests.
pub fn verify(manifest: &BackupManifest, computed: &[FileDigest]) -> VerifyReport {
    let mut report = VerifyReport::default();
    for expected in &manifest.files {
        match computed.iter().find(|c| c.path == expected.path) {
            None => report.missing.push(expected.path.clone()),
            Some(actual) => {
                if actual.sha256 != expected.sha256 || actual.size != expected.size {
                    report.changed.push(expected.path.clone());
                }
            }
        }
    }
    for actual in computed {
        if manifest.file(&actual.path).is_none() {
            report.extra.push(actual.path.clone());
        }
    }
    report
}

/// Snapshot metadata (used by the retention policy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub kind: SnapshotKind,
    pub created_at: crate::Timestamp,
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Number of rolling snapshots to keep per kind.
    pub keep_rolling: usize,
    /// Total byte cap (`None` = unlimited). When exceeded, the oldest rolling snapshots are pruned
    /// first.
    pub max_bytes: Option<u64>,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            keep_rolling: 5,
            max_bytes: Some(4 * 1024 * 1024),
        }
    }
}

impl RetentionPolicy {
    /// Return the snapshot paths that need to be deleted. **`initial` is never pruned.**
    pub fn plan_prune(&self, snapshots: &[SnapshotMeta]) -> Vec<String> {
        let mut doomed: Vec<String> = Vec::new();

        // 1) Group by kind and keep the newest keep_rolling of each
        for kind in [SnapshotKind::PreStart, SnapshotKind::PreChange] {
            let mut group: Vec<&SnapshotMeta> =
                snapshots.iter().filter(|s| s.kind == kind).collect();
            group.sort_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| b.path.cmp(&a.path))
            });
            for meta in group.into_iter().skip(self.keep_rolling) {
                doomed.push(meta.path.clone());
            }
        }

        // 2) Total cap: keep pruning from the oldest rolling snapshots
        if let Some(max) = self.max_bytes {
            let mut remaining: Vec<&SnapshotMeta> = snapshots
                .iter()
                .filter(|s| !s.kind.is_immutable())
                .filter(|s| !doomed.contains(&s.path))
                .collect();
            remaining.sort_by_key(|item| item.created_at);
            let mut total: u64 = snapshots.iter().map(|s| s.bytes).sum();
            for meta in remaining {
                if total <= max {
                    break;
                }
                total = total.saturating_sub(meta.bytes);
                doomed.push(meta.path.clone());
            }
        }

        doomed.sort();
        doomed.dedup();
        doomed
    }
}

/// Restore scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreMode {
    /// Default: restore only the keys wifisync changed (other user settings are kept).
    #[default]
    ManagedOnly,
    /// Overwrite everything from the baseline (requires a second confirmation).
    Full,
}

/// Restore action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum RestoreAction {
    /// Replace the whole file with the snapshot version.
    ReplaceFile { file: String, from: String },
    /// Restore a key to its original value.
    SetOption { key: String, value: String },
    /// Delete a key added by wifisync that did not exist in the original configuration.
    DeleteOption { key: String },
    /// Reload the network after restoring.
    ReloadNetwork,
    /// Reload wireless after restoring.
    ReloadWifi,
}

impl RestoreAction {
    pub fn describe(&self) -> String {
        match self {
            RestoreAction::ReplaceFile { file, from } => {
                format!(
                    "overwrite /etc/config/{} from the snapshot ({})",
                    file, from
                )
            }
            RestoreAction::SetOption { key, value } => format!("{} = {}", key, value),
            RestoreAction::DeleteOption { key } => format!("delete {}", key),
            RestoreAction::ReloadNetwork => "reload network (ubus call network reload)".to_string(),
            RestoreAction::ReloadWifi => "reload wireless (wifi reload)".to_string(),
        }
    }
}

/// Comparison input for a single managed key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedEntry {
    /// `network.lan.ipaddr`
    pub key: String,
    /// Original value in the snapshot (`None` = the key did not exist in the original
    /// configuration)
    pub original: Option<String>,
    /// Current value
    pub current: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorePlan {
    pub mode: RestoreMode,
    pub actions: Vec<RestoreAction>,
    /// Explicitly skipped items and the reason (e.g. keys the user changed that are not in the
    /// managed list).
    pub skipped: Vec<String>,
    /// Non-empty means execution is refused.
    pub blocked: Option<String>,
}

impl RestorePlan {
    pub fn is_blocked(&self) -> bool {
        self.blocked.is_some()
    }

    pub fn summary(&self) -> String {
        format!(
            "{:?} restore plan: {} action(s), {} skipped{}",
            self.mode,
            self.actions.len(),
            self.skipped.len(),
            self.blocked
                .as_ref()
                .map(|r| format!(", blocked: {}", r))
                .unwrap_or_default()
        )
    }
}

/// Build a restore plan.
///
/// * `mode = ManagedOnly`: only handle `entries` whose current value differs from the original;
/// * `mode = Full`: overwrite all files in `manifest.files`;
/// * In any case, a failed verification becomes `blocked` (restore is refused; the baseline is
///   never deleted).
pub fn build_restore_plan(
    mode: RestoreMode,
    manifest: &BackupManifest,
    snapshot_dir: &str,
    verification: &VerifyReport,
    entries: &[ManagedEntry],
) -> RestorePlan {
    if !verification.is_ok() {
        return RestorePlan {
            mode,
            actions: Vec::new(),
            skipped: Vec::new(),
            blocked: Some(format!(
                "{} (baseline directory {})",
                verification.summary(),
                snapshot_dir
            )),
        };
    }

    let mut actions = Vec::new();
    let mut skipped = Vec::new();

    match mode {
        RestoreMode::Full => {
            for file in &manifest.files {
                actions.push(RestoreAction::ReplaceFile {
                    file: file.path.clone(),
                    from: snapshot_dir.to_string(),
                });
            }
            skipped.push(
                "full overwrite mode: all user changes in those files are reverted".to_string(),
            );
        }
        RestoreMode::ManagedOnly => {
            for entry in entries {
                match (&entry.original, &entry.current) {
                    (Some(original), Some(current)) if original == current => {
                        skipped.push(format!("{} unchanged", entry.key));
                    }
                    (Some(original), _) => actions.push(RestoreAction::SetOption {
                        key: entry.key.clone(),
                        value: original.clone(),
                    }),
                    (None, Some(_)) => actions.push(RestoreAction::DeleteOption {
                        key: entry.key.clone(),
                    }),
                    (None, None) => skipped.push(format!("{} absent", entry.key)),
                }
            }
        }
    }

    if !actions.is_empty() {
        let touches_wireless = actions.iter().any(|a| match a {
            RestoreAction::SetOption { key, .. } | RestoreAction::DeleteOption { key } => {
                key.starts_with("wireless.")
            }
            RestoreAction::ReplaceFile { file, .. } => file == "wireless",
            _ => false,
        });
        if touches_wireless {
            actions.push(RestoreAction::ReloadWifi);
        }
        actions.push(RestoreAction::ReloadNetwork);
    }

    RestorePlan {
        mode,
        actions,
        skipped,
        blocked: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(path: &str, hash: &str) -> FileDigest {
        FileDigest {
            path: path.to_string(),
            sha256: hash.to_string(),
            size: 10,
        }
    }

    fn manifest() -> BackupManifest {
        let mut m = BackupManifest::new(
            SnapshotKind::Initial,
            1000,
            "dev-1",
            Roles::default_for(&Default::default()),
        );
        m.files = vec![
            digest("config/network", "aaa"),
            digest("config/wireless", "bbb"),
        ];
        m.managed_keys = vec!["network.lan.ipaddr".into()];
        m
    }

    #[test]
    fn verify_detects_missing_changed_and_extra() {
        let m = manifest();
        let computed = vec![
            digest("config/network", "zzz"), // changed
            digest("config/dhcp", "ccc"),    // extra
        ];
        let report = verify(&m, &computed);
        assert!(!report.is_ok());
        assert_eq!(report.changed, vec!["config/network"]);
        assert_eq!(report.missing, vec!["config/wireless"]);
        assert_eq!(report.extra, vec!["config/dhcp"]);
    }

    #[test]
    fn initial_snapshot_is_never_pruned() {
        let policy = RetentionPolicy {
            keep_rolling: 1,
            max_bytes: Some(1),
        };
        let snaps = vec![
            SnapshotMeta {
                kind: SnapshotKind::Initial,
                created_at: 1,
                path: "initial".into(),
                bytes: 10_000,
            },
            SnapshotMeta {
                kind: SnapshotKind::PreStart,
                created_at: 2,
                path: "pre-start-2".into(),
                bytes: 10_000,
            },
            SnapshotMeta {
                kind: SnapshotKind::PreStart,
                created_at: 3,
                path: "pre-start-3".into(),
                bytes: 10_000,
            },
        ];
        let doomed = policy.plan_prune(&snaps);
        assert!(!doomed.contains(&"initial".to_string()));
        assert!(doomed.contains(&"pre-start-2".to_string()));
    }

    #[test]
    fn managed_only_restore_touches_only_recorded_keys() {
        let m = manifest();
        let entries = vec![
            ManagedEntry {
                key: "network.lan.ipaddr".into(),
                original: Some("192.168.1.1".into()),
                current: Some("10.0.0.1".into()),
            },
            ManagedEntry {
                key: "network.lan.netmask".into(),
                original: Some("255.255.255.0".into()),
                current: Some("255.255.255.0".into()),
            },
        ];
        let plan = build_restore_plan(
            RestoreMode::ManagedOnly,
            &m,
            "/etc/wifisync/backup/initial",
            &VerifyReport::default(),
            &entries,
        );
        assert!(!plan.is_blocked());
        assert_eq!(
            plan.actions,
            vec![
                RestoreAction::SetOption {
                    key: "network.lan.ipaddr".into(),
                    value: "192.168.1.1".into()
                },
                RestoreAction::ReloadNetwork
            ]
        );
        assert_eq!(plan.skipped, vec!["network.lan.netmask unchanged"]);
    }

    #[test]
    fn managed_only_deletes_keys_created_by_wifisync() {
        let m = manifest();
        let entries = vec![ManagedEntry {
            key: "network.br_lan_device".into(),
            original: None,
            current: Some("br-lan".into()),
        }];
        let plan = build_restore_plan(
            RestoreMode::ManagedOnly,
            &m,
            "/snap",
            &VerifyReport::default(),
            &entries,
        );
        assert_eq!(
            plan.actions,
            vec![
                RestoreAction::DeleteOption {
                    key: "network.br_lan_device".into()
                },
                RestoreAction::ReloadNetwork
            ]
        );
    }

    #[test]
    fn wireless_restore_triggers_wifi_reload() {
        let m = manifest();
        let entries = vec![ManagedEntry {
            key: "wireless.radio0.channel".into(),
            original: Some("36".into()),
            current: Some("44".into()),
        }];
        let plan = build_restore_plan(
            RestoreMode::ManagedOnly,
            &m,
            "/snap",
            &VerifyReport::default(),
            &entries,
        );
        assert!(plan.actions.contains(&RestoreAction::ReloadWifi));
        assert!(plan.actions.contains(&RestoreAction::ReloadNetwork));
    }

    #[test]
    fn verification_failure_blocks_restore() {
        let m = manifest();
        let report = VerifyReport {
            changed: vec!["config/network".into()],
            ..Default::default()
        };
        let plan = build_restore_plan(RestoreMode::Full, &m, "/snap", &report, &[]);
        assert!(plan.is_blocked());
        assert!(plan.actions.is_empty());
        assert!(plan.summary().contains("blocked"));
    }

    #[test]
    fn full_restore_replaces_all_files() {
        let m = manifest();
        let plan = build_restore_plan(
            RestoreMode::Full,
            &m,
            "/etc/wifisync/backup/initial",
            &VerifyReport::default(),
            &[],
        );
        assert_eq!(plan.actions.len(), 3); // 2 files + ReloadNetwork
        assert!(matches!(plan.actions[0], RestoreAction::ReplaceFile { .. }));
    }
}
