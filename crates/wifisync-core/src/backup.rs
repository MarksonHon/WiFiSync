//! 备份 / 快照 / 恢复的**模型与决策**（需求 11）。
//!
//! 纪律：**启动先备份初始化网络，停止先恢复原有网络。**
//!
//! 这里只做纯计算：
//! * [`BackupManifest`]：快照清单（含每文件 sha256 与 `managed_keys`）
//! * [`RetentionPolicy`]：保留策略（`initial` 永不裁剪）
//! * [`build_restore_plan`]：恢复计划（默认 `managed_only`，只动 wifisync 改过的键）

use crate::role::Roles;
use serde::{Deserialize, Serialize};

/// 快照格式版本。
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    /// 不可变的初始化基线（权威还原源）。
    Initial,
    /// 每次服务启动前的滚动快照。
    PreStart,
    /// 每次写入前的滚动快照。
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
    /// 相对快照目录的路径，如 `config/network`。
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// 快照清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub format_version: u32,
    pub kind: SnapshotKind,
    pub created_at: crate::Timestamp,
    pub device_id: String,
    pub wifisync_version: String,
    /// 建立快照时的角色集合（用于恢复后的一致性提示）。
    pub roles: Roles,
    pub files: Vec<FileDigest>,
    /// 由 wifisync 改动过的 uci 键路径（`network.lan.ipaddr` 形式）。
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
            return "备份完整性校验通过".to_string();
        }
        format!(
            "备份校验失败：缺失 {} 项、内容变化 {} 项、多出 {} 项",
            self.missing.len(),
            self.changed.len(),
            self.extra.len()
        )
    }
}

/// 对比清单与实际计算出的摘要。
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

/// 快照元信息（用于保留策略）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub kind: SnapshotKind,
    pub created_at: crate::Timestamp,
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// 滚动快照各保留份数。
    pub keep_rolling: usize,
    /// 总字节上限（`None` = 不限制）。超限时优先裁剪最旧的滚动快照。
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
    /// 返回需要删除的快照路径。**`initial` 永不裁剪。**
    pub fn plan_prune(&self, snapshots: &[SnapshotMeta]) -> Vec<String> {
        let mut doomed: Vec<String> = Vec::new();

        // 1) 按 kind 分组，保留每类最新的 keep_rolling 份
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

        // 2) 总量上限：继续从最旧的滚动快照开始裁
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

/// 恢复范围。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreMode {
    /// 默认：只还原 wifisync 改动过的键（用户其它配置保留）。
    #[default]
    ManagedOnly,
    /// 按基线整体覆盖（需要二次确认）。
    Full,
}

/// 恢复动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum RestoreAction {
    /// 用快照文件整体替换。
    ReplaceFile { file: String, from: String },
    /// 把某个键还原为原值。
    SetOption { key: String, value: String },
    /// 删除由 wifisync 新增、原配置中不存在的键。
    DeleteOption { key: String },
    /// 恢复后重载网络。
    ReloadNetwork,
    /// 恢复后重载无线。
    ReloadWifi,
}

impl RestoreAction {
    pub fn describe(&self) -> String {
        match self {
            RestoreAction::ReplaceFile { file, from } => {
                format!("用快照覆盖 /etc/config/{}（来自 {}）", file, from)
            }
            RestoreAction::SetOption { key, value } => format!("{} = {}", key, value),
            RestoreAction::DeleteOption { key } => format!("删除 {}", key),
            RestoreAction::ReloadNetwork => "重载网络（ubus call network reload）".to_string(),
            RestoreAction::ReloadWifi => "重载无线（wifi reload）".to_string(),
        }
    }
}

/// 单个受管键的对比输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedEntry {
    /// `network.lan.ipaddr`
    pub key: String,
    /// 快照里的原值（`None` = 原配置中不存在该键）
    pub original: Option<String>,
    /// 当前值
    pub current: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorePlan {
    pub mode: RestoreMode,
    pub actions: Vec<RestoreAction>,
    /// 明确跳过的项与原因（例如用户自己改过、但不在受管列表里的键）。
    pub skipped: Vec<String>,
    /// 非空表示拒绝执行。
    pub blocked: Option<String>,
}

impl RestorePlan {
    pub fn is_blocked(&self) -> bool {
        self.blocked.is_some()
    }

    pub fn summary(&self) -> String {
        format!(
            "{:?} 恢复计划：{} 个动作，跳过 {} 项{}",
            self.mode,
            self.actions.len(),
            self.skipped.len(),
            self.blocked
                .as_ref()
                .map(|r| format!("，已阻止：{}", r))
                .unwrap_or_default()
        )
    }
}

/// 生成恢复计划。
///
/// * `mode = ManagedOnly`：只处理 `entries` 中「当前值 != 原值」的键；
/// * `mode = Full`：整体覆盖 `manifest.files` 中的文件；
/// * 任何情况下，校验失败都会 `blocked`（拒绝恢复，绝不删基线）。
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
                "{}（基线目录 {}）",
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
            skipped.push("整体覆盖模式：用户在该文件中的所有改动都会被还原".to_string());
        }
        RestoreMode::ManagedOnly => {
            for entry in entries {
                match (&entry.original, &entry.current) {
                    (Some(original), Some(current)) if original == current => {
                        skipped.push(format!("{} 未变化", entry.key));
                    }
                    (Some(original), _) => actions.push(RestoreAction::SetOption {
                        key: entry.key.clone(),
                        value: original.clone(),
                    }),
                    (None, Some(_)) => actions.push(RestoreAction::DeleteOption {
                        key: entry.key.clone(),
                    }),
                    (None, None) => skipped.push(format!("{} 不存在", entry.key)),
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
        assert_eq!(plan.skipped, vec!["network.lan.netmask 未变化"]);
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
        assert!(plan.summary().contains("已阻止"));
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
        assert_eq!(plan.actions.len(), 3); // 2 文件 + ReloadNetwork
        assert!(matches!(plan.actions[0], RestoreAction::ReplaceFile { .. }));
    }
}
