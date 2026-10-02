//! 运行时状态：device-id、准入登记簿、档案版本、脏标记。
//!
//! 状态文件一律用「临时文件 + rename」原子写入，避免掉电写坏。

use crate::error::SysResult;
use crate::paths::Paths;
use serde_json::{json, Value};
use std::io::Write;
use wifisync_core::admission::AdmissionRegistry;

pub struct StateStore {
    paths: Paths,
}

impl StateStore {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    /// 读取（或首次生成）设备 ID。
    pub fn device_id(&self) -> SysResult<String> {
        let path = self.paths.device_id_file();
        if let Ok(text) = std::fs::read_to_string(&path) {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }
        let id = random_hex(16)?;
        self.paths.ensure_dir(&self.paths.persistent_dir())?;
        write_atomic(&path, id.as_bytes())?;
        Ok(id)
    }

    /// 读取状态 JSON（不存在则返回默认结构）。
    pub fn load(&self) -> SysResult<Value> {
        let path = self.paths.state_file();
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(_) => Ok(json!({
                "profile_version": 0,
                "admissions": { "entries": [] },
            })),
        }
    }

    pub fn save(&self, value: &Value) -> SysResult<()> {
        self.paths.ensure_dir(&self.paths.persistent_dir())?;
        let text = serde_json::to_string_pretty(value)?;
        write_atomic(&self.paths.state_file(), text.as_bytes())
    }

    pub fn admissions(&self) -> SysResult<AdmissionRegistry> {
        let value = self.load()?;
        let registry = value
            .get("admissions")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        Ok(registry)
    }

    pub fn save_admissions(&self, registry: &AdmissionRegistry) -> SysResult<()> {
        let mut value = self.load()?;
        value["admissions"] = serde_json::to_value(registry)?;
        self.save(&value)
    }

    pub fn profile_version(&self) -> SysResult<u64> {
        Ok(self
            .load()?
            .get("profile_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0))
    }

    /// 递增并返回新的档案版本号（下发给 AP 的 `NetworkProfile.version`）。
    pub fn bump_profile_version(&self) -> SysResult<u64> {
        let mut value = self.load()?;
        let next = value
            .get("profile_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            + 1;
        value["profile_version"] = json!(next);
        self.save(&value)?;
        Ok(next)
    }

    // ── 脏标记：异常退出（kill -9 / 断电）后由下次启动识别 ──────────────────

    pub fn mark_dirty(&self, reason: &str) -> SysResult<()> {
        self.paths.ensure_dir(&self.paths.run_dir())?;
        write_atomic(&self.paths.dirty_marker(), reason.as_bytes())
    }

    pub fn clear_dirty(&self) -> SysResult<()> {
        let path = self.paths.dirty_marker();
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn dirty_reason(&self) -> Option<String> {
        std::fs::read_to_string(self.paths.dirty_marker())
            .ok()
            .map(|s| s.trim().to_string())
    }
}

/// 从 `/dev/urandom` 取随机字节并转十六进制。
pub fn random_hex(bytes: usize) -> SysResult<String> {
    use std::io::Read;
    let mut buf = vec![0u8; bytes];
    let mut file = std::fs::File::open("/dev/urandom")?;
    file.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{:02x}", b)).collect())
}

/// 原子写文件（同目录临时文件 + rename）。
pub fn write_atomic(path: &std::path::Path, content: &[u8]) -> SysResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(content)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_paths(tag: &str) -> (Paths, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("wifisync-state-{}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Paths::with_root(&dir), dir)
    }

    #[test]
    fn device_id_is_stable() {
        let (paths, dir) = tmp_paths("devid");
        let store = StateStore::new(paths);
        let first = store.device_id().unwrap();
        let second = store.device_id().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 32);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn profile_version_increments() {
        let (paths, dir) = tmp_paths("version");
        let store = StateStore::new(paths);
        assert_eq!(store.profile_version().unwrap(), 0);
        assert_eq!(store.bump_profile_version().unwrap(), 1);
        assert_eq!(store.bump_profile_version().unwrap(), 2);
        assert_eq!(store.profile_version().unwrap(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn admissions_roundtrip() {
        let (paths, dir) = tmp_paths("adm");
        let store = StateStore::new(paths);
        let mut registry = AdmissionRegistry::default();
        registry.register(wifisync_core::admission::AdmissionEntry::new(
            "dev-1",
            "aa:bb:cc:dd:ee:ff",
            10,
        ));
        registry.approve("dev-1", 20).unwrap();
        store.save_admissions(&registry).unwrap();
        let loaded = store.admissions().unwrap();
        assert!(loaded.require_approved("dev-1").is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dirty_marker_lifecycle() {
        let (paths, dir) = tmp_paths("dirty");
        let store = StateStore::new(paths);
        assert!(store.dirty_reason().is_none());
        store.mark_dirty("running").unwrap();
        assert_eq!(store.dirty_reason().as_deref(), Some("running"));
        store.clear_dirty().unwrap();
        assert!(store.dirty_reason().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
