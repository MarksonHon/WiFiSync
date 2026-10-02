//! Resident service: lifecycle (**back up on start / restore on stop**), admission, plan delivery,
//! and the watchdog.

use serde_json::{json, Value};
use std::sync::{Arc, Mutex, RwLock};
use wifisync_core::admission::{AdmissionEntry, AdmissionRegistry, AdmissionState};
use wifisync_core::backup::{RestoreMode, SnapshotKind};
use wifisync_core::bridge::{bridge_disabled_reason, plan_bridges, BridgePlanInput};
use wifisync_core::capability::Capabilities;
use wifisync_core::config::WifisyncConfig;
use wifisync_core::failsafe::{Decision, Failsafe, FailsafeEvent};
use wifisync_core::plan::{build_write_plan, PlanInput, WritePlan};
use wifisync_core::profile::NetworkProfile;
use wifisync_core::role::Roles;
use wifisync_core::wifi_source::{self, ResolvedWifi, SourceContext, WifiSourceConfig};
use wifisync_core::Message;
use wifisync_sys::error::SysResult;
use wifisync_sys::restore::Restorer;
use wifisync_sys::snapshot::{now, SnapshotStore};
use wifisync_sys::state::StateStore;
use wifisync_sys::uci::Uci;
use wifisync_sys::Paths;

use crate::{log_error, log_info, log_warn, probe, rpc, signals};

#[derive(Debug, Clone, Default)]
pub struct DaemonOptions {
    /// Do not restore on stop (escape hatch; must be explicitly requested by the user).
    pub no_restore_on_stop: bool,
    /// Emit logs as JSON (easier for LuCI to parse).
    pub json_log: bool,
}

pub struct Daemon {
    paths: Paths,
    uci: Uci,
    state: StateStore,
    snapshots: SnapshotStore,
    restorer: Restorer,
    config: RwLock<WifisyncConfig>,
    caps: RwLock<Capabilities>,
    failsafe: Mutex<Failsafe>,
    admissions: Mutex<AdmissionRegistry>,
    profile: Mutex<NetworkProfile>,
    options: DaemonOptions,
}

impl Daemon {
    pub fn new(paths: Paths, options: DaemonOptions) -> SysResult<Self> {
        let uci = Uci::new(paths.clone());
        let state = StateStore::new(paths.clone());
        let snapshots = SnapshotStore::new(paths.clone());
        let restorer = Restorer::new(paths.clone());
        let mut config = uci.load_config()?;
        if config.device_id.is_empty() {
            config.device_id = state.device_id()?;
        }
        let wireless_text = uci.read_config_file("wireless")?;
        let mut caps = wifisync_sys::sysfs::capabilities(&paths, wireless_text.as_deref())?;
        caps.radios.sort_by(|a, b| a.name.cmp(&b.name));

        // Requirement 3: with no configured roles, take defaults from the hardware capabilities:
        // wireless present => controller + ap + gateway; absent => controller + gateway (no AP).
        if !config.roles_configured {
            config.roles = Roles::default_for(&caps);
            config.roles_configured = true;
            if config.device_id.is_empty() {
                config.device_id = state.device_id()?;
            }
            // A failed first write (e.g. no uci on the host) must not stop the service, only warn
            let ops = config.to_uci_ops();
            if let Err(e) = uci.apply_ops(&ops) {
                log_warn!(
                    "failed to write the default role configuration ({:?}): {}",
                    config.roles,
                    e
                );
            } else {
                log_info!(
                    "roles initialized from the hardware capabilities: {} ({})",
                    config.roles.to_uci_value(),
                    if caps.has_wifi() {
                        "wireless module detected"
                    } else {
                        "no wireless module detected, AP role dropped"
                    }
                );
            }
        }

        // Drop AP when the hardware does not support it (belt and braces for requirement 3)
        let adjustments = config.roles.sanitize(&caps);
        if !adjustments.is_empty() {
            log_warn!("roles adjusted due to hardware limits: {:?}", adjustments);
        }
        let admissions = state.admissions().unwrap_or_default();
        let failsafe = Failsafe::for_roles(config.failsafe.clone(), &config.roles);

        Ok(Self {
            paths,
            uci,
            state,
            snapshots,
            restorer,
            config: RwLock::new(config),
            caps: RwLock::new(caps),
            failsafe: Mutex::new(failsafe),
            admissions: Mutex::new(admissions),
            profile: Mutex::new(NetworkProfile::default()),
            options,
        })
    }

    // ── Lifecycle ──────────────────────────────────────────────────────────

    /// Start: **the initial baseline must first be created/verified successfully**, otherwise
    /// refuse to start (fail-closed).
    pub fn start(self: Arc<Self>) -> SysResult<()> {
        let cfg = self.config_snapshot();
        log_info!(
            "starting wifisync {} (device {}, roles [{}])",
            wifisync_core::VERSION,
            cfg.device_id,
            cfg.roles.to_uci_value()
        );

        self.paths.ensure_dir(&self.paths.persistent_dir())?;
        self.paths.ensure_dir(&self.paths.run_dir())?;

        // 0. Dirty-shutdown detection
        if let Some(reason) = self.state.dirty_reason() {
            log_warn!(
                "previous shutdown was not clean ({}); check the network state, the initial baseline is available for recovery",
                reason
            );
        }

        // 1. Initial baseline (immutable)
        let (manifest, created) = self.snapshots.ensure_initial(&cfg)?;
        if created {
            log_info!(
                "initial network baseline created ({} files, {})",
                manifest.files.len(),
                manifest.created_at
            );
        } else {
            let report = self.snapshots.verify(&self.paths.initial_snapshot_dir())?;
            if report.is_ok() {
                log_info!("initial baseline verification passed");
            } else {
                log_warn!("initial baseline verification failed: {}", report.summary());
            }
        }

        // 2. Rolling snapshot for this start
        match self.snapshots.create(SnapshotKind::PreStart, &cfg, &[]) {
            Ok(snapshot) => log_info!("pre-start snapshot created ({})", snapshot.created_at),
            Err(e) => log_warn!("creating the pre-start snapshot failed: {}", e),
        }
        if let Err(e) = self
            .snapshots
            .prune(&wifisync_core::backup::RetentionPolicy::default())
        {
            log_warn!("pruning snapshots failed: {}", e);
        }

        // 3. Dirty flag + socket
        self.state.mark_dirty("running")?;
        let server = rpc::Server::bind(&self.paths.socket_file())?;
        log_info!("listening on {}", server.socket_path().display());

        // 4. Watchdog (only effective for AP; Failsafe decides internally)
        self.spawn_watchdog();

        // 5. Service loop: RPC on its own thread, main thread waits for the shutdown signal
        let handler_daemon = Arc::clone(&self);
        let serving = std::thread::spawn(move || {
            let result = server.serve(move |method, params| handler_daemon.handle(method, params));
            if let Err(e) = result {
                log_error!("RPC server exited: {}", e);
            }
        });

        while !signals::shutdown_requested() {
            if signals::take_reload_request() {
                log_info!("SIGHUP received, reloading the configuration");
                if let Err(e) = self.reload() {
                    log_error!("reloading the configuration failed: {}", e);
                }
            }
            signals::sleep_interruptible(500);
        }

        log_info!("shutdown signal received, cleaning up");
        let _ = serving; // the accept loop ends when the process exits
        self.stop()?;
        Ok(())
    }

    /// Stop: **restore the original network first, then exit**.
    pub fn stop(&self) -> SysResult<()> {
        if self.options.no_restore_on_stop {
            log_warn!("--no-restore-on-stop is enabled: the network configuration is not restored on this stop (explicitly requested)");
        } else {
            let cfg = self.config_snapshot();
            let dir = self.paths.initial_snapshot_dir();
            if dir.join("manifest.json").exists() {
                match self.restorer.plan(cfg.restore_mode, &dir) {
                    Ok(plan) => {
                        if plan.is_blocked() {
                            log_error!("pre-stop restore blocked: {}", plan.summary());
                        } else {
                            match self.restorer.execute(&plan, &dir) {
                                Ok(report) => {
                                    if report.success() {
                                        log_info!(
                                            "previous network restored before stopping: {}",
                                            report.summary()
                                        );
                                    } else {
                                        log_error!(
                                            "pre-stop restore had problems: {}",
                                            report.summary()
                                        );
                                        let _ = wifisync_sys::netifd::restart_network();
                                    }
                                }
                                Err(e) => {
                                    log_error!(
                                        "pre-stop restore failed: {}, restarting the network",
                                        e
                                    );
                                    let _ = wifisync_sys::netifd::restart_network();
                                }
                            }
                        }
                    }
                    Err(e) => log_error!("building the restore plan failed: {}", e),
                }
            } else {
                log_warn!("no initial baseline, skipping the pre-stop restore");
            }
        }

        let _ = self.state.clear_dirty();
        let socket = self.paths.socket_file();
        if socket.exists() {
            let _ = std::fs::remove_file(socket);
        }
        log_info!("wifisync stopped");
        Ok(())
    }

    /// Reload the configuration and capabilities (called after SIGHUP / `roles.set`).
    pub fn reload(&self) -> SysResult<()> {
        let mut config = self.uci.load_config()?;
        if config.device_id.is_empty() {
            config.device_id = self.state.device_id()?;
        }
        let wireless_text = self.uci.read_config_file("wireless")?;
        let caps = wifisync_sys::sysfs::capabilities(&self.paths, wireless_text.as_deref())?;
        let adjustments = {
            let mut cfg = config.clone();
            cfg.roles.sanitize(&caps)
        };
        *self.caps.write().unwrap() = caps;
        let failsafe_config = config.failsafe.clone();
        let roles = config.roles;
        *self.config.write().unwrap() = config;
        let mut failsafe = self.failsafe.lock().unwrap();
        failsafe.set_config(failsafe_config);
        if !adjustments.is_empty() {
            log_warn!(
                "roles from the configuration adjusted due to hardware limits ({:?})",
                adjustments
            );
        }
        failsafe.on_event(FailsafeEvent::Confirmed, now());
        let _ = roles;
        Ok(())
    }

    fn spawn_watchdog(self: &Arc<Self>) {
        let daemon = Arc::clone(self);
        std::thread::spawn(move || {
            log_info!("watchdog started (5 s interval)");
            while !signals::shutdown_requested() {
                signals::sleep_interruptible(5_000);
                if signals::shutdown_requested() {
                    break;
                }
                daemon.watchdog_tick();
            }
        });
    }

    fn watchdog_tick(&self) {
        let cfg = self.config_snapshot();

        // L2: heartbeat probe (only meaningful for AP with failsafe enabled)
        if cfg.failsafe.enabled && cfg.roles.ap {
            if let Some(endpoint) = cfg.failsafe.heartbeat_endpoint.clone() {
                let reachable = probe::probe(&endpoint).reachable;
                let mut failsafe = self.failsafe.lock().unwrap();
                let event = if reachable {
                    FailsafeEvent::HeartbeatOk
                } else {
                    FailsafeEvent::HeartbeatLost
                };
                failsafe.on_event(event, now());
            }
        }

        let decision = {
            let mut failsafe = self.failsafe.lock().unwrap();
            failsafe.tick(now())
        };
        if let Some(decision) = decision {
            log_warn!("failover triggered: {}", decision.reason);
            if let Err(e) = self.perform_recovery(&decision) {
                log_error!("performing the failover recovery failed: {}", e);
            }
        }
    }

    fn perform_recovery(&self, decision: &Decision) -> SysResult<()> {
        let dir = self.paths.initial_snapshot_dir();
        let cfg = self.config_snapshot();
        let plan = self.restorer.plan(cfg.restore_mode, &dir)?;
        if plan.is_blocked() {
            log_error!("failover recovery blocked: {}", plan.summary());
        } else {
            let report = self.restorer.execute(&plan, &dir)?;
            log_info!("failover recovery result: {}", report.summary());
        }
        if decision.reboot {
            log_warn!("rebooting the device as configured");
            let _ = wifisync_sys::netifd::reboot();
        }
        Ok(())
    }

    // ── Plan and write ────────────────────────────────────────────────────────

    pub fn config_snapshot(&self) -> WifisyncConfig {
        self.config.read().unwrap().clone()
    }

    pub fn caps_snapshot(&self) -> Capabilities {
        self.caps.read().unwrap().clone()
    }

    /// Resolve the Wi-Fi information source (may return user-readable errors such as
    /// "source unavailable").
    pub fn resolve_wifi(&self) -> Result<Option<ResolvedWifi>, String> {
        let cfg = self.config_snapshot();
        if !cfg.roles.ap {
            return Ok(None);
        }
        let ctx = self.source_context(&cfg);
        match wifi_source::resolve(&cfg.wifi_source, &ctx) {
            Ok(resolved) => Ok(Some(resolved)),
            Err(e) => Err(e.to_string()),
        }
    }

    fn source_context(&self, cfg: &WifisyncConfig) -> SourceContext {
        let state = self.state.load().unwrap_or_else(|_| json!({}));
        SourceContext {
            local_is_ap: cfg.roles.ap,
            controller_reports_wifi: self.caps_snapshot().has_wifi(),
            gateway_reports_wifi: state.get("gateway_reports_wifi").and_then(|v| v.as_bool()),
            gateway_profile_fetched: state
                .get("gateway_profile_fetched")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            local_wifi_change_confirmed: cfg.local_wifi_change_confirmed,
            gateway_profile: None,
        }
    }

    /// Build the current write plan (**the only** source of network writes).
    pub fn current_plan(&self) -> Result<WritePlan, String> {
        let cfg = self.config_snapshot();
        let caps = self.caps_snapshot();
        let wifi = self.resolve_wifi()?;
        let profile = self.profile.lock().unwrap().clone();
        Ok(build_write_plan(&PlanInput {
            roles: cfg.roles,
            caps: &caps,
            bridge_name: &cfg.bridge_name,
            extra_bridges: Vec::new(),
            wifi: wifi.as_ref(),
            profile: Some(&profile),
        }))
    }

    /// Apply the write plan: touch the system only when non-empty, and arm the apply-guard.
    pub fn apply(&self) -> Result<Value, String> {
        let cfg = self.config_snapshot();

        // Admission check: a pure AP device must be approved (requirement 8)
        if cfg.roles.ap && !cfg.roles.controller {
            let admissions = self.admissions.lock().unwrap().clone();
            admissions
                .require_approved(&cfg.device_id)
                .map_err(|e| e.to_string())?;
        }

        let plan = self.current_plan()?;
        if plan.is_empty() {
            // Zero-intrusion path: no uci write, no reload
            return Ok(json!({
                "changed": 0,
                "plan": plan.dry_run_text(),
                "notes": plan.notes,
                "message": Message::new("daemon.apply.nothing_to_do"),
            }));
        }

        // Pre-write snapshot + record the managed keys (for precise restore)
        let snapshot = self
            .snapshots
            .create(SnapshotKind::PreChange, &cfg, &plan.managed_keys())
            .map_err(|e| e.to_string())?;
        let snapshot_dir = self
            .snapshots
            .dir_for(SnapshotKind::PreChange, snapshot.created_at);
        let _ = self.snapshots.write_last_change_marker(&snapshot_dir);

        // Arm the dead-man timer
        let guard = {
            let mut failsafe = self.failsafe.lock().unwrap();
            failsafe.on_event(FailsafeEvent::ApplyStarted, now());
            failsafe.config().clone()
        };

        // The actual write
        let executed = self
            .uci
            .apply_ops(&plan.ops)
            .map_err(|e| format!("writing uci failed: {}", e))?;
        if plan.reload_wifi {
            wifisync_sys::netifd::reload_wifi()
                .map_err(|e| format!("reloading wireless failed: {}", e))?;
        } else if plan.reload_network {
            wifisync_sys::netifd::reload_network()
                .map_err(|e| format!("reloading the network failed: {}", e))?;
        }

        Ok(json!({
            "changed": plan.ops.len(),
            "executed": executed,
            "plan": plan.dry_run_text(),
            "notes": plan.notes,
            "snapshot": snapshot_dir.to_string_lossy(),
            "confirm_deadline_secs": guard.apply_confirm_secs,
            "message": Message::new("daemon.apply.applied")
                .param("count", plan.ops.len().to_string()),
        }))
    }

    /// Confirm a successful apply (releases the apply-guard).
    pub fn confirm(&self) -> Value {
        let mut failsafe = self.failsafe.lock().unwrap();
        let armed = failsafe.state().clone();
        failsafe.on_event(FailsafeEvent::Confirmed, now());
        json!({
            "confirmed": true,
            "previous_state": format!("{:?}", armed),
            "state": format!("{:?}", failsafe.state()),
        })
    }

    /// Roll back to the most recent pre-write snapshot (L1).
    pub fn revert_last_change(&self) -> Result<Value, String> {
        let Some(dir) = self.snapshots.last_change_dir() else {
            return Err("no pre-change snapshot available".to_string());
        };
        let cfg = self.config_snapshot();
        let plan = self
            .restorer
            .plan(cfg.restore_mode, &dir)
            .map_err(|e| e.to_string())?;
        if plan.is_blocked() {
            return Err(plan.summary());
        }
        let report = self
            .restorer
            .execute(&plan, &dir)
            .map_err(|e| e.to_string())?;
        {
            let mut failsafe = self.failsafe.lock().unwrap();
            failsafe.on_event(FailsafeEvent::Confirmed, now());
        }
        Ok(json!({
            "snapshot": dir.to_string_lossy(),
            "report": report.summary_message(),
            "applied": report.applied,
        }))
    }

    /// Restore from the given snapshot (the initial baseline by default).
    pub fn restore(
        &self,
        mode: Option<RestoreMode>,
        snapshot: Option<String>,
    ) -> Result<Value, String> {
        let cfg = self.config_snapshot();
        let mode = mode.unwrap_or(cfg.restore_mode);
        let dir = match snapshot.as_deref() {
            None | Some("initial") => self.paths.initial_snapshot_dir(),
            Some(path) => std::path::PathBuf::from(path),
        };
        if !dir.exists() {
            return Err(format!(
                "snapshot directory does not exist: {}",
                dir.display()
            ));
        }
        let plan = self.restorer.plan(mode, &dir).map_err(|e| e.to_string())?;
        if plan.is_blocked() {
            return Err(plan.summary());
        }
        let report = self
            .restorer
            .execute(&plan, &dir)
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "mode": format!("{:?}", mode),
            "snapshot": dir.to_string_lossy(),
            "report": report.summary_message(),
            "applied": report.applied,
            "skipped": plan.skipped,
        }))
    }

    // ── RPC dispatch ──────────────────────────────────────────────────────────

    pub fn handle(&self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "version" => Ok(json!({
                "version": wifisync_core::VERSION,
                "protocol": 1,
                "features": ["kvr", "admission", "backup", "failsafe", "multi_bridge", "vlan"],
            })),
            "status" => Ok(self.status()),
            "capabilities" => Ok(serde_json::to_value(self.caps_snapshot()).unwrap_or(Value::Null)),
            "roles.get" => Ok(json!({
                "roles": self.config_snapshot().roles,
                "value": self.config_snapshot().roles.to_uci_value(),
                "labels": self.config_snapshot().roles.labels(),
                "bridge_enabled": self.config_snapshot().roles.enable_bridge(),
                "bridge_blocked_reason": bridge_disabled_reason(&self.config_snapshot().roles),
                "ap_allowed": self.caps_snapshot().has_wifi(),
            })),
            "roles.set" => self.rpc_roles_set(params),
            "bridge.preview" => self.rpc_bridge_preview(),
            "plan.dry_run" => {
                let plan = self.current_plan()?;
                Ok(json!({
                    "empty": plan.is_empty(),
                    "text": plan.dry_run_text(),
                    "ops": plan.ops,
                    "notes": plan.notes,
                    "bridges": plan.bridges,
                    "wifi_radios": plan.wifi_radios,
                    "managed_keys": plan.managed_keys(),
                    "summary": plan.summary(),
                }))
            }
            "apply" => self.apply(),
            "confirm" => Ok(self.confirm()),
            "revert_last_change" => self.revert_last_change(),
            "restore" => {
                let mode = params
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .map(|m| match m {
                        "full" => RestoreMode::Full,
                        _ => RestoreMode::ManagedOnly,
                    });
                let snapshot = params
                    .get("snapshot")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                self.restore(mode, snapshot)
            }
            "wifi_source.get" => self.rpc_wifi_source_get(),
            "wifi_source.set" => self.rpc_wifi_source_set(params),
            "gateway.set" => self.rpc_gateway_set(params),
            "probe.connectivity" => {
                let target = params
                    .get("target")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing target argument".to_string())?;
                Ok(probe::probe(target).to_json())
            }
            "admission.list" => {
                let registry = self.admissions.lock().unwrap();
                Ok(json!({
                    "entries": registry.entries(),
                    "pending": registry.pending_count(),
                    "approved": registry.by_state(AdmissionState::Approved).len(),
                    "rejected": registry.by_state(AdmissionState::Rejected).len(),
                }))
            }
            "admission.register" => self.rpc_admission_register(params),
            "admission.approve" => {
                self.rpc_admission_state(params, |reg, id| reg.approve(id, now()))
            }
            "admission.reject" => self.rpc_admission_state(params, |reg, id| reg.reject(id, now())),
            "admission.revoke" => self.rpc_admission_state(params, |reg, id| reg.revoke(id, now())),
            "backup.list" => {
                let items = self.snapshots.list().map_err(rpc::sys_err)?;
                let initial = self.paths.initial_snapshot_dir();
                Ok(json!({
                    "initial_exists": self.snapshots.initial_exists(),
                    "initial_dir": initial.to_string_lossy(),
                    "snapshots": items,
                }))
            }
            "backup.verify" => {
                // With no path given: verify the immutable baseline first, then the latest snapshot
                let dir = match params.get("path").and_then(|v| v.as_str()) {
                    Some(path) => std::path::PathBuf::from(path),
                    None => {
                        let initial = self.paths.initial_snapshot_dir();
                        if initial.join("manifest.json").exists() {
                            initial
                        } else {
                            let items = self.snapshots.list().map_err(rpc::sys_err)?;
                            items
                                .last()
                                .map(|meta| std::path::PathBuf::from(meta.path.clone()))
                                .ok_or_else(|| {
                                    Message::new("daemon.backup.no_snapshot").to_string()
                                })?
                        }
                    }
                };
                let report = self.snapshots.verify(&dir).map_err(rpc::sys_err)?;
                Ok(json!({
                    "ok": report.is_ok(),
                    "path": dir.to_string_lossy(),
                    "summary": report.summary_message(),
                    "missing": report.missing,
                    "changed": report.changed,
                    "extra": report.extra,
                }))
            }
            "backup.create" => {
                let cfg = self.config_snapshot();
                let kind = match params.get("kind").and_then(|v| v.as_str()) {
                    Some("pre-change") => SnapshotKind::PreChange,
                    _ => SnapshotKind::PreStart,
                };
                let plan = self.current_plan()?;
                let manifest = self
                    .snapshots
                    .create(kind, &cfg, &plan.managed_keys())
                    .map_err(rpc::sys_err)?;
                Ok(json!({
                    "created_at": manifest.created_at,
                    "kind": format!("{:?}", manifest.kind),
                    "files": manifest.files.len(),
                    "managed_keys": manifest.managed_keys,
                }))
            }
            "backup.prune" => {
                let doomed = self
                    .snapshots
                    .prune(&wifisync_core::backup::RetentionPolicy::default())
                    .map_err(rpc::sys_err)?;
                Ok(json!({ "removed": doomed }))
            }
            "failsafe.get" => {
                let failsafe = self.failsafe.lock().unwrap();
                Ok(json!({
                    "config": failsafe.config(),
                    "state": failsafe.state(),
                    "state_label": failsafe.state().label(),
                    "applies": failsafe.applies(),
                    "active": failsafe.is_active(),
                }))
            }
            "failsafe.set" => self.rpc_failsafe_set(params),
            "profile.get" => {
                let profile = self.profile.lock().unwrap().clone();
                Ok(serde_json::to_value(profile).unwrap_or(Value::Null))
            }
            "profile.publish" => self.rpc_profile_publish(params),
            "logs.tail" => {
                let lines = params.get("lines").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
                let path = self.paths.log_file();
                let content = std::fs::read_to_string(path).unwrap_or_default();
                let tail: Vec<&str> = content.lines().rev().take(lines).collect();
                Ok(json!({ "lines": tail.into_iter().rev().collect::<Vec<_>>() }))
            }
            other => Err(format!("unknown method `{}`", other)),
        }
    }

    fn status(&self) -> Value {
        let cfg = self.config_snapshot();
        let caps = self.caps_snapshot();
        let failsafe = self.failsafe.lock().unwrap();
        let admissions = self.admissions.lock().unwrap();
        let plan = self.current_plan().unwrap_or_default();
        let wifi_source_error = self.resolve_wifi().err();

        json!({
            "version": wifisync_core::VERSION,
            "device_id": cfg.device_id,
            "roles": cfg.roles,
            "role_labels": cfg.roles.labels(),
            "bridge_enabled": cfg.roles.enable_bridge(),
            "bridge_blocked_reason": bridge_disabled_reason(&cfg.roles),
            "capabilities": caps,
            "wifi_source": {
                "kind": cfg.wifi_source.kind,
                "error": wifi_source_error,
            },
            "failsafe": {
                "state": failsafe.state(),
                "state_label": failsafe.state().label(),
                "applies": failsafe.applies(),
                "active": failsafe.is_active(),
                "config": failsafe.config(),
            },
            "admission": {
                "pending": admissions.pending_count(),
                "approved": admissions.by_state(AdmissionState::Approved).len(),
                "rejected": admissions.by_state(AdmissionState::Rejected).len(),
            },
            "plan": {
                "empty": plan.is_empty(),
                "summary": plan.summary(),
                "managed_keys": plan.managed_keys(),
            },
            "backup": {
                "initial_exists": self.snapshots.initial_exists(),
                "initial_dir": self.paths.initial_snapshot_dir().to_string_lossy(),
                "dirty": self.state.dirty_reason().is_some(),
                "dirty_reason": self.state.dirty_reason(),
            },
            "sync_mode": cfg.sync_mode,
            "gateway_lan_ifaces": cfg.gateway_lan_ifaces,
            "restore_mode": cfg.restore_mode,
            "socket": self.paths.socket_file().to_string_lossy(),
        })
    }

    fn rpc_roles_set(&self, params: &Value) -> Result<Value, String> {
        let raw = params
            .get("roles")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing roles argument (e.g. \"controller ap\")".to_string())?;
        let mut roles = Roles::from_uci_value(raw).map_err(|e| e.to_string())?;
        let caps = self.caps_snapshot();
        if roles.ap && !caps.has_wifi() {
            return Err(
                "this device has no wireless module, the AP role cannot be selected".to_string(),
            );
        }
        let adjustments = roles.sanitize(&caps);
        roles.validate(&caps).map_err(|e| e.to_string())?;

        {
            let mut cfg = self.config.write().unwrap();
            cfg.roles = roles;
            cfg.to_uci_ops();
        }
        self.persist_config()?;
        {
            let cfg = self.config_snapshot();
            let mut failsafe = self.failsafe.lock().unwrap();
            *failsafe = Failsafe::for_roles(cfg.failsafe.clone(), &cfg.roles);
        }
        Ok(json!({
            "roles": roles,
            "value": roles.to_uci_value(),
            "bridge_enabled": roles.enable_bridge(),
            "adjustments": adjustments.iter().map(|a| a.message()).collect::<Vec<_>>(),
            "plan_reload_required": true,
        }))
    }

    fn rpc_bridge_preview(&self) -> Result<Value, String> {
        let cfg = self.config_snapshot();
        let caps = self.caps_snapshot();
        let bridges = plan_bridges(&BridgePlanInput {
            roles: cfg.roles,
            caps: &caps,
            bridge_name: &cfg.bridge_name,
            extra: Vec::new(),
        });
        Ok(json!({
            "enabled": cfg.roles.enable_bridge(),
            "reason": bridge_disabled_reason(&cfg.roles),
            "bridges": bridges,
            "ports": caps.port_names(),
            "lan_ports": caps.lan_ports(),
            "wan_ports": caps.wan_ports(),
            "bridge_name": cfg.bridge_name,
        }))
    }

    fn rpc_wifi_source_get(&self) -> Result<Value, String> {
        let cfg = self.config_snapshot();
        let ctx = self.source_context(&cfg);
        let availability = wifi_source::source_availability(&cfg.wifi_source, &ctx);
        Ok(json!({
            "config": cfg.wifi_source,
            "availability": availability.iter().map(|(kind, enabled, reason)| json!({
                "kind": kind.as_str(),
                "enabled": enabled,
                "reason": reason,
            })).collect::<Vec<_>>(),
            "local_is_ap": cfg.roles.ap,
            "local_wifi_change_confirmed": cfg.local_wifi_change_confirmed,
            "resolved": self.resolve_wifi().ok().flatten(),
            "error": self.resolve_wifi().err(),
            "kvr": {
                "available": self.caps_snapshot().kvr_ready(),
                "note": wifi_source::kvr_available(&self.caps_snapshot()).1,
            },
        }))
    }

    fn rpc_wifi_source_set(&self, params: &Value) -> Result<Value, String> {
        let kind = params
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing kind argument".to_string())?;
        let kind = wifi_source::WifiSourceKind::from_str_opt(kind)
            .ok_or_else(|| format!("unknown source `{}`", kind))?;

        let confirmed = params
            .get("local_wifi_change_confirmed")
            .and_then(|v| v.as_bool());

        {
            let mut cfg = self.config.write().unwrap();
            let mut source: WifiSourceConfig = cfg.wifi_source.clone();
            source.kind = kind;
            if let Some(endpoint) = params.get("gateway_endpoint").and_then(|v| v.as_str()) {
                source.gateway_endpoint = if endpoint.is_empty() {
                    None
                } else {
                    Some(endpoint.to_string())
                };
            }
            if let Some(custom) = params.get("custom") {
                let mut value = source.custom.clone().unwrap_or_default();
                if let Some(ssid) = custom.get("ssid").and_then(|v| v.as_str()) {
                    value.ssid = ssid.to_string();
                }
                if let Some(auth) = custom.get("auth").and_then(|v| v.as_str()) {
                    value.auth = auth.to_string();
                }
                if let Some(psk_ref) = custom.get("psk_ref").and_then(|v| v.as_str()) {
                    value.psk_ref = psk_ref.to_string();
                }
                if let Some(band) = custom.get("band").and_then(|v| v.as_str()) {
                    value.band = band.to_string();
                }
                if let Some(channel) = custom.get("channel").and_then(|v| v.as_u64()) {
                    value.channel = Some(channel as u32);
                }
                if let Some(domain) = custom.get("mobility_domain").and_then(|v| v.as_str()) {
                    value.kvr.mobility_domain = domain.to_string();
                }
                for (key, target) in [
                    ("ieee80211k", &mut value.kvr.k),
                    ("ieee80211v", &mut value.kvr.v),
                    ("ieee80211r", &mut value.kvr.r),
                    ("ft_over_ds", &mut value.kvr.ft_over_ds),
                ] {
                    if let Some(flag) = custom.get(key).and_then(|v| v.as_bool()) {
                        *target = flag;
                    }
                }
                source.custom = Some(value);
            }
            cfg.wifi_source = source;
            if let Some(confirmed) = confirmed {
                cfg.local_wifi_change_confirmed = confirmed;
            }
        }

        // Validation: custom + local AP requires explicit confirmation
        let cfg = self.config_snapshot();
        cfg.validate(&self.caps_snapshot())
            .map_err(|e| e.to_string())?;
        let preview = self.current_plan()?;
        self.persist_config()?;

        Ok(json!({
            "saved": true,
            "requires_confirmation": cfg.roles.ap
                && cfg.wifi_source.kind == wifi_source::WifiSourceKind::Custom
                && !cfg.local_wifi_change_confirmed,
            "plan": preview.dry_run_text(),
        }))
    }

    fn rpc_gateway_set(&self, params: &Value) -> Result<Value, String> {
        let interfaces: Vec<String> = params
            .get("lan_ifaces")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let caps = self.caps_snapshot();
        for iface in &interfaces {
            if !caps.port_names().contains(iface) {
                return Err(format!(
                    "interface `{}` is not in the local port list",
                    iface
                ));
            }
        }
        {
            let mut cfg = self.config.write().unwrap();
            cfg.gateway_lan_ifaces = interfaces.clone();
        }
        self.persist_config()?;
        let plan = self.current_plan()?;
        Ok(json!({
            "saved": true,
            "lan_ifaces": interfaces,
            "network_modified": !plan.is_empty(),
            "message": Message::new("daemon.gateway.recorded"),
        }))
    }

    fn rpc_admission_register(&self, params: &Value) -> Result<Value, String> {
        let device_id = params
            .get("device_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing device_id".to_string())?;
        let mac = params.get("mac").and_then(|v| v.as_str()).unwrap_or("");
        let mut entry = AdmissionEntry::new(device_id, mac, now());
        entry.hostname = params
            .get("hostname")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        entry.source_addr = params
            .get("source_addr")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        entry.model = params
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        entry.radios = params.get("radios").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        let state = {
            let mut registry = self.admissions.lock().unwrap();
            registry.register(entry)
        };
        let registry = self.admissions.lock().unwrap().clone();
        self.state
            .save_admissions(&registry)
            .map_err(rpc::sys_err)?;
        Ok(json!({ "device_id": device_id, "state": state }))
    }

    fn rpc_admission_state<F>(&self, params: &Value, action: F) -> Result<Value, String>
    where
        F: FnOnce(&mut AdmissionRegistry, &str) -> wifisync_core::CoreResult<()>,
    {
        let device_id = params
            .get("device_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing device_id".to_string())?;
        {
            let mut registry = self.admissions.lock().unwrap();
            action(&mut registry, device_id).map_err(|e| e.to_string())?;
        }
        let registry = self.admissions.lock().unwrap().clone();
        self.state
            .save_admissions(&registry)
            .map_err(rpc::sys_err)?;
        Ok(json!({
            "device_id": device_id,
            "state": registry.find(device_id).map(|e| e.state),
        }))
    }

    fn rpc_failsafe_set(&self, params: &Value) -> Result<Value, String> {
        let mut config = self.config_snapshot().failsafe;
        if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
            config.enabled = enabled;
        }
        if let Some(secs) = params.get("link_timeout_secs").and_then(|v| v.as_u64()) {
            config.link_timeout_secs = secs;
        }
        if let Some(secs) = params.get("apply_confirm_secs").and_then(|v| v.as_u64()) {
            config.apply_confirm_secs = secs;
        }
        if let Some(action) = params.get("action").and_then(|v| v.as_str()) {
            config.action = match action {
                "reboot" => wifisync_core::failsafe::FailsafeAction::Reboot,
                _ => wifisync_core::failsafe::FailsafeAction::Revert,
            };
        }
        if let Some(keep) = params.get("keep_ssid").and_then(|v| v.as_bool()) {
            config.keep_ssid = keep;
        }
        if let Some(endpoint) = params.get("heartbeat_endpoint").and_then(|v| v.as_str()) {
            config.heartbeat_endpoint = if endpoint.is_empty() {
                None
            } else {
                Some(endpoint.to_string())
            };
        }
        {
            let mut cfg = self.config.write().unwrap();
            cfg.failsafe = config.clone();
        }
        let cfg = self.config_snapshot();
        cfg.validate(&self.caps_snapshot())
            .map_err(|e| e.to_string())?;
        self.persist_config()?;
        {
            let mut failsafe = self.failsafe.lock().unwrap();
            *failsafe = Failsafe::for_roles(config.clone(), &cfg.roles);
        }
        Ok(json!({ "saved": true, "config": config }))
    }

    fn rpc_profile_publish(&self, params: &Value) -> Result<Value, String> {
        let incoming: NetworkProfile = serde_json::from_value(
            params
                .get("profile")
                .cloned()
                .ok_or_else(|| "missing profile".to_string())?,
        )
        .map_err(|e| format!("parsing the profile failed: {}", e))?;
        incoming.validate()?;

        let cfg = self.config_snapshot();
        if !cfg.roles.controller {
            return Err("this device does not take the Controller role and cannot distribute network information".to_string());
        }
        let version = self.state.bump_profile_version().map_err(rpc::sys_err)?;
        let merged = {
            let mut current = self.profile.lock().unwrap();
            let mut next = incoming.clone();
            next.version = version;
            next.updated_at = now();
            let merged = current.merge(&next);
            *current = merged.clone();
            merged
        };
        Ok(json!({ "version": merged.version, "profile": merged }))
    }

    fn persist_config(&self) -> Result<(), String> {
        let cfg = self.config_snapshot();
        cfg.validate(&self.caps_snapshot())
            .map_err(|e| e.to_string())?;
        let ops = cfg.to_uci_ops();
        self.uci.apply_ops(&ops).map(|_| ()).map_err(rpc::sys_err)
    }
}
