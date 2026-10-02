//! Failover state machine (requirement 6 / 11).
//!
//! Two levels of guard rails, both pure state machines, with time injected by the caller:
//!
//! * **L1 apply-guard**: after applying a configuration it must be confirmed within
//!   `apply_confirm_secs`, otherwise it rolls back automatically;
//! * **L2 link-watchdog**: **AP devices only**; if the heartbeat with the Controller/Gateway is
//!   lost for more than `link_timeout_secs`, restore the device default network according to
//!   `action` (optionally rebooting).
//!
//! Gateway / Controller do not participate (zero intrusion), so [`Failsafe::for_roles`] marks
//! non-AP devices as "not applicable" right away.

use crate::message::Message;
use crate::role::Roles;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailsafeAction {
    /// Restore the device default network configuration.
    #[default]
    Revert,
    /// Restore the default network, then reboot.
    Reboot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailsafeConfig {
    /// Requirement 6 calls this an "optional setting"; it is off by default.
    pub enabled: bool,
    /// apply-guard confirmation window.
    pub apply_confirm_secs: u64,
    /// How long the heartbeat may be lost before recovery triggers.
    pub link_timeout_secs: u64,
    pub action: FailsafeAction,
    /// Keep the current SSID when restoring the default network (so neighbouring devices do not
    /// lose all contact).
    pub keep_ssid: bool,
    /// Heartbeat endpoint (Controller or Gateway address).
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
    /// A new network configuration is about to be applied (arm the dead-man timer).
    ApplyStarted,
    /// The user/upper layer confirms the apply succeeded.
    Confirmed,
    HeartbeatOk,
    HeartbeatLost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum FailsafeState {
    /// Feature is off.
    Disabled,
    /// Not applicable to this device (not an AP).
    NotApplicable,
    Idle,
    /// apply-guard is waiting for confirmation.
    PendingConfirm {
        since: crate::Timestamp,
    },
    /// Waiting for the heartbeat to recover.
    Watchdog {
        since: crate::Timestamp,
    },
    /// Tripped (fires only once per run).
    Tripped {
        at: crate::Timestamp,
        reason: String,
    },
}

impl FailsafeState {
    /// UI message describing this state.
    pub fn label(&self) -> Message {
        match self {
            FailsafeState::Disabled => Message::new("failsafe.state.disabled"),
            FailsafeState::NotApplicable => Message::new("failsafe.state.not_applicable"),
            FailsafeState::Idle => Message::new("failsafe.state.idle"),
            FailsafeState::PendingConfirm { .. } => Message::new("failsafe.state.pending_confirm"),
            FailsafeState::Watchdog { .. } => Message::new("failsafe.state.watchdog"),
            FailsafeState::Tripped { .. } => Message::new("failsafe.state.tripped"),
        }
    }
}

/// The decision produced by the state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub action: FailsafeAction,
    pub reason: String,
    /// Whether the decision produced by the state machine requires keeping the SSID.
    pub keep_ssid: bool,
    /// Whether to reboot after tripping.
    pub reboot: bool,
}

pub struct Failsafe {
    config: FailsafeConfig,
    applies: bool,
    state: FailsafeState,
}

impl Failsafe {
    /// Create from config; when `applies=false` (not an AP) the state is
    /// [`FailsafeState::NotApplicable`].
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

    /// Convenience constructor: applicable only to the AP role.
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
            // Already tripped; do not fire again during this run
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

    /// Tick driver; returns the action to execute.
    pub fn tick(&mut self, now: crate::Timestamp) -> Option<Decision> {
        if !self.is_active() {
            return None;
        }
        match self.state.clone() {
            FailsafeState::PendingConfirm { since } => {
                let elapsed = now.saturating_sub(since).max(0) as u64;
                if elapsed >= self.config.apply_confirm_secs {
                    let reason = format!(
                        "not confirmed within {} s after applying the configuration; rolling back to the pre-apply snapshot",
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
                        "heartbeat with the Controller/Gateway lost for more than {} s; restoring the default network",
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
        assert!(
            fs.tick(1_020).is_none(),
            "must not trigger before the confirmation window elapsed"
        );
        let decision = fs.tick(1_031).unwrap();
        assert_eq!(decision.action, FailsafeAction::Revert);
        assert!(!decision.reboot);
        assert!(decision.reason.contains("not confirmed"));
        // Fires only once
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
        assert!(decision.reason.contains("heartbeat"));
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
