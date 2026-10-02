//! 故障恢复状态机（需求 6 / 11）。
//!
//! 两级护栏，都是纯状态机，时间由调用方注入：
//!
//! * **L1 apply-guard**：应用配置后必须在 `apply_confirm_secs` 内确认，否则自动回滚；
//! * **L2 link-watchdog**：**仅 AP 设备**，与 Controller/Gateway 心跳丢失超过
//!   `link_timeout_secs` 就按 `action` 恢复设备默认网络（可选重启）。
//!
//! Gateway / Controller 不参与（零侵入），因此 [`Failsafe::for_roles`] 会把非 AP 设备
//! 直接置为「不适用」。

use crate::role::Roles;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailsafeAction {
    /// 恢复设备默认网络配置。
    #[default]
    Revert,
    /// 恢复默认网络后重启。
    Reboot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailsafeConfig {
    /// 需求 6 明确为“可选设置”，默认关闭。
    pub enabled: bool,
    /// apply-guard 确认窗口。
    pub apply_confirm_secs: u64,
    /// 心跳丢失多久后触发恢复。
    pub link_timeout_secs: u64,
    pub action: FailsafeAction,
    /// 恢复默认网络时保留当前 SSID（避免邻居设备彻底失联）。
    pub keep_ssid: bool,
    /// 心跳端点（Controller 或 Gateway 地址）。
    pub heartbeat_endpoint: Option<String>,
}

impl Default for FailsafeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            apply_confirm_secs: 90,
            link_timeout_secs: 300,
            action: FailsafeAction::Revert,
            keep_ssid: false,
            heartbeat_endpoint: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailsafeEvent {
    /// 即将应用一份新的网络配置（武装死手定时器）。
    ApplyStarted,
    /// 用户/上层确认应用成功。
    Confirmed,
    HeartbeatOk,
    HeartbeatLost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum FailsafeState {
    /// 功能关闭。
    Disabled,
    /// 本设备不适用（非 AP）。
    NotApplicable,
    Idle,
    /// apply-guard 等待确认。
    PendingConfirm {
        since: crate::Timestamp,
    },
    /// 等待心跳恢复。
    Watchdog {
        since: crate::Timestamp,
    },
    /// 已触发（本次运行内只触发一次）。
    Tripped {
        at: crate::Timestamp,
        reason: String,
    },
}

impl FailsafeState {
    pub fn label(&self) -> &'static str {
        match self {
            FailsafeState::Disabled => "已关闭",
            FailsafeState::NotApplicable => "不适用（非 AP 设备）",
            FailsafeState::Idle => "监控中",
            FailsafeState::PendingConfirm { .. } => "等待应用确认",
            FailsafeState::Watchdog { .. } => "等待心跳恢复",
            FailsafeState::Tripped { .. } => "已触发恢复",
        }
    }
}

/// 状态机给出的决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub action: FailsafeAction,
    pub reason: String,
    /// 由状态机产生的决定是否要求保留 SSID。
    pub keep_ssid: bool,
    /// 触发后是否重启。
    pub reboot: bool,
}

pub struct Failsafe {
    config: FailsafeConfig,
    applies: bool,
    state: FailsafeState,
}

impl Failsafe {
    /// 按配置创建；`applies=false`（非 AP）时状态为 [`FailsafeState::NotApplicable`]。
    pub fn new(config: FailsafeConfig, applies: bool) -> Self {
        let state = if !applies {
            FailsafeState::NotApplicable
        } else if config.enabled {
            FailsafeState::Idle
        } else {
            FailsafeState::Disabled
        };
        Self {
            config,
            applies,
            state,
        }
    }

    /// 便捷构造：只有 AP 角色才适用。
    pub fn for_roles(config: FailsafeConfig, roles: &Roles) -> Self {
        Self::new(config, roles.ap)
    }

    pub fn config(&self) -> &FailsafeConfig {
        &self.config
    }

    pub fn state(&self) -> &FailsafeState {
        &self.state
    }

    pub fn applies(&self) -> bool {
        self.applies
    }

    pub fn is_active(&self) -> bool {
        self.applies && self.config.enabled
    }

    pub fn set_config(&mut self, config: FailsafeConfig) {
        self.config = config;
        if !self.applies {
            self.state = FailsafeState::NotApplicable;
        } else if !self.config.enabled {
            self.state = FailsafeState::Disabled;
        } else if matches!(
            self.state,
            FailsafeState::Disabled | FailsafeState::NotApplicable
        ) {
            self.state = FailsafeState::Idle;
        }
    }

    pub fn on_event(&mut self, event: FailsafeEvent, now: crate::Timestamp) -> Option<Decision> {
        if !self.is_active() {
            return None;
        }
        if let FailsafeState::Tripped { .. } = self.state {
            // 已触发，本次运行不再重复触发
            return None;
        }
        match event {
            FailsafeEvent::ApplyStarted => {
                self.state = FailsafeState::PendingConfirm { since: now };
                None
            }
            FailsafeEvent::Confirmed => {
                self.state = FailsafeState::Idle;
                None
            }
            FailsafeEvent::HeartbeatOk => {
                if let FailsafeState::Watchdog { .. } = self.state {
                    self.state = FailsafeState::Idle;
                }
                None
            }
            FailsafeEvent::HeartbeatLost => {
                if let FailsafeState::Watchdog { .. } = self.state {
                    return None;
                }
                self.state = FailsafeState::Watchdog { since: now };
                None
            }
        }
    }

    /// 定时驱动；返回需要执行的动作。
    pub fn tick(&mut self, now: crate::Timestamp) -> Option<Decision> {
        if !self.is_active() {
            return None;
        }
        match self.state.clone() {
            FailsafeState::PendingConfirm { since } => {
                let elapsed = now.saturating_sub(since).max(0) as u64;
                if elapsed >= self.config.apply_confirm_secs {
                    let reason = format!(
                        "配置应用后在 {} 秒内未确认，自动回滚到应用前快照",
                        self.config.apply_confirm_secs
                    );
                    self.state = FailsafeState::Tripped {
                        at: now,
                        reason: reason.clone(),
                    };
                    return Some(Decision {
                        action: FailsafeAction::Revert,
                        reason,
                        keep_ssid: false,
                        reboot: false,
                    });
                }
                None
            }
            FailsafeState::Watchdog { since } => {
                let elapsed = now.saturating_sub(since).max(0) as u64;
                if elapsed >= self.config.link_timeout_secs {
                    let reason = format!(
                        "与 Controller/Gateway 的心跳丢失超过 {} 秒，恢复设备默认网络",
                        self.config.link_timeout_secs
                    );
                    self.state = FailsafeState::Tripped {
                        at: now,
                        reason: reason.clone(),
                    };
                    return Some(Decision {
                        action: self.config.action,
                        reason,
                        keep_ssid: self.config.keep_ssid,
                        reboot: self.config.action == FailsafeAction::Reboot,
                    });
                }
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled() -> FailsafeConfig {
        FailsafeConfig {
            enabled: true,
            apply_confirm_secs: 30,
            link_timeout_secs: 120,
            ..Default::default()
        }
    }

    #[test]
    fn disabled_by_default() {
        let fs = Failsafe::new(FailsafeConfig::default(), true);
        assert!(!fs.is_active());
        assert_eq!(*fs.state(), FailsafeState::Disabled);
    }

    #[test]
    fn not_applicable_for_non_ap() {
        let fs = Failsafe::for_roles(enabled(), &Roles::controller_only());
        assert!(!fs.applies());
        assert_eq!(*fs.state(), FailsafeState::NotApplicable);
    }

    #[test]
    fn apply_guard_trips_when_not_confirmed() {
        let mut fs = Failsafe::new(enabled(), true);
        assert!(fs.on_event(FailsafeEvent::ApplyStarted, 1_000).is_none());
        assert!(fs.tick(1_020).is_none(), "未到确认窗口不应触发");
        let decision = fs.tick(1_031).unwrap();
        assert_eq!(decision.action, FailsafeAction::Revert);
        assert!(!decision.reboot);
        assert!(decision.reason.contains("未确认"));
        // 只触发一次
        assert!(fs.tick(2_000).is_none());
    }

    #[test]
    fn confirm_disarms_apply_guard() {
        let mut fs = Failsafe::new(enabled(), true);
        fs.on_event(FailsafeEvent::ApplyStarted, 100);
        fs.on_event(FailsafeEvent::Confirmed, 110);
        assert!(fs.tick(1_000).is_none());
        assert_eq!(*fs.state(), FailsafeState::Idle);
    }

    #[test]
    fn heartbeat_loss_trips_after_timeout() {
        let mut fs = Failsafe::new(enabled(), true);
        fs.on_event(FailsafeEvent::HeartbeatLost, 500);
        assert_eq!(fs.tick(600), None);
        let decision = fs.tick(620).unwrap();
        assert_eq!(decision.action, FailsafeAction::Revert);
        assert!(decision.reason.contains("心跳丢失"));
    }

    #[test]
    fn heartbeat_recovery_cancels_watchdog() {
        let mut fs = Failsafe::new(enabled(), true);
        fs.on_event(FailsafeEvent::HeartbeatLost, 500);
        fs.on_event(FailsafeEvent::HeartbeatOk, 510);
        assert!(fs.tick(10_000).is_none());
        assert_eq!(*fs.state(), FailsafeState::Idle);
    }

    #[test]
    fn reboot_action_sets_reboot_flag() {
        let mut fs = Failsafe::new(
            FailsafeConfig {
                action: FailsafeAction::Reboot,
                ..enabled()
            },
            true,
        );
        fs.on_event(FailsafeEvent::HeartbeatLost, 0);
        let decision = fs.tick(1_000).unwrap();
        assert!(decision.reboot);
    }

    #[test]
    fn disabling_at_runtime_resets_state() {
        let mut fs = Failsafe::new(enabled(), true);
        fs.on_event(FailsafeEvent::ApplyStarted, 10);
        fs.set_config(FailsafeConfig::default());
        assert_eq!(*fs.state(), FailsafeState::Disabled);
        assert!(fs.tick(99_999).is_none());
    }
}
