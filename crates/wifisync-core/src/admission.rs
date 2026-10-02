//! 新 AP 准入（需求 8）。
//!
//! Controller 对 AP 只做两件事：**验证（准入）** 与 **信息下发**。
//! 未通过准入的 AP 不得写入本机任何配置。

use crate::error::{CoreError, CoreResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionState {
    /// 已上报，等待管理员批准。
    Pending,
    Approved,
    /// 被拒绝并进入黑名单。
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionEntry {
    /// 设备持久化 UUID。
    pub device_id: String,
    pub mac: String,
    pub hostname: Option<String>,
    /// 上报时的来源地址（用于展示与排查）。
    pub source_addr: Option<String>,
    pub model: Option<String>,
    pub radios: u32,
    pub first_seen: crate::Timestamp,
    pub last_seen: crate::Timestamp,
    pub state: AdmissionState,
    /// 管理员备注。
    pub note: Option<String>,
}

impl AdmissionEntry {
    pub fn new(
        device_id: impl Into<String>,
        mac: impl Into<String>,
        now: crate::Timestamp,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            mac: mac.into(),
            hostname: None,
            source_addr: None,
            model: None,
            radios: 0,
            first_seen: now,
            last_seen: now,
            state: AdmissionState::Pending,
            note: None,
        }
    }

    pub fn is_approved(&self) -> bool {
        self.state == AdmissionState::Approved
    }
}

/// 准入登记簿。纯数据结构，持久化由 `wifisync-sys` 负责。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionRegistry {
    entries: Vec<AdmissionEntry>,
}

impl AdmissionRegistry {
    pub fn entries(&self) -> &[AdmissionEntry] {
        &self.entries
    }

    pub fn by_state(&self, state: AdmissionState) -> Vec<&AdmissionEntry> {
        self.entries.iter().filter(|e| e.state == state).collect()
    }

    pub fn find(&self, device_id: &str) -> Option<&AdmissionEntry> {
        self.entries.iter().find(|e| e.device_id == device_id)
    }

    pub fn pending_count(&self) -> usize {
        self.by_state(AdmissionState::Pending).len()
    }

    /// 设备上报：已存在则刷新 `last_seen`（不改变状态，黑名单不会被自动重置）。
    pub fn register(&mut self, mut entry: AdmissionEntry) -> AdmissionState {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.device_id == entry.device_id)
        {
            existing.last_seen = entry.last_seen;
            existing.source_addr = entry.source_addr.take().or(existing.source_addr.take());
            existing.radios = entry.radios;
            return existing.state;
        }
        let state = entry.state;
        self.entries.push(entry);
        self.entries.sort_by(|a, b| {
            a.first_seen
                .cmp(&b.first_seen)
                .then_with(|| a.device_id.cmp(&b.device_id))
        });
        state
    }

    pub fn approve(&mut self, device_id: &str, now: crate::Timestamp) -> CoreResult<()> {
        self.set_state(device_id, AdmissionState::Approved, now)
    }

    pub fn reject(&mut self, device_id: &str, now: crate::Timestamp) -> CoreResult<()> {
        self.set_state(device_id, AdmissionState::Rejected, now)
    }

    /// 撤销批准：回到待批准状态。
    pub fn revoke(&mut self, device_id: &str, now: crate::Timestamp) -> CoreResult<()> {
        self.set_state(device_id, AdmissionState::Pending, now)
    }

    fn set_state(
        &mut self,
        device_id: &str,
        state: AdmissionState,
        now: crate::Timestamp,
    ) -> CoreResult<()> {
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.device_id == device_id)
            .ok_or_else(|| CoreError::NotAdmitted(device_id.to_string()))?;
        entry.state = state;
        entry.last_seen = now;
        Ok(())
    }

    /// 写入前的硬性检查：未批准一律拒绝（防绕过 LuCI）。
    pub fn require_approved(&self, device_id: &str) -> CoreResult<()> {
        match self.find(device_id) {
            Some(entry) if entry.is_approved() => Ok(()),
            _ => Err(CoreError::NotAdmitted(device_id.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, now: i64) -> AdmissionEntry {
        AdmissionEntry::new(id, "aa:bb:cc:dd:ee:ff", now)
    }

    #[test]
    fn new_device_is_pending() {
        let mut reg = AdmissionRegistry::default();
        assert_eq!(reg.register(entry("dev-1", 100)), AdmissionState::Pending);
        assert_eq!(reg.pending_count(), 1);
        assert!(reg.require_approved("dev-1").is_err());
    }

    #[test]
    fn approve_then_write_is_allowed() {
        let mut reg = AdmissionRegistry::default();
        reg.register(entry("dev-1", 100));
        reg.approve("dev-1", 200).unwrap();
        assert!(reg.require_approved("dev-1").is_ok());
        assert_eq!(reg.pending_count(), 0);
    }

    #[test]
    fn rejected_device_stays_rejected_after_reregister() {
        let mut reg = AdmissionRegistry::default();
        reg.register(entry("dev-1", 100));
        reg.reject("dev-1", 150).unwrap();
        assert_eq!(reg.register(entry("dev-1", 300)), AdmissionState::Rejected);
        assert!(reg.require_approved("dev-1").is_err());
        assert_eq!(reg.by_state(AdmissionState::Rejected).len(), 1);
    }

    #[test]
    fn revoke_moves_back_to_pending() {
        let mut reg = AdmissionRegistry::default();
        reg.register(entry("dev-1", 100));
        reg.approve("dev-1", 120).unwrap();
        reg.revoke("dev-1", 130).unwrap();
        assert_eq!(reg.find("dev-1").unwrap().state, AdmissionState::Pending);
    }

    #[test]
    fn unknown_device_operations_fail() {
        let mut reg = AdmissionRegistry::default();
        assert!(reg.approve("nope", 1).is_err());
        assert!(reg.require_approved("nope").is_err());
    }

    #[test]
    fn registry_is_deterministically_ordered() {
        let mut reg = AdmissionRegistry::default();
        reg.register(entry("b", 20));
        reg.register(entry("a", 10));
        let ids: Vec<&str> = reg.entries().iter().map(|e| e.device_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }
}
