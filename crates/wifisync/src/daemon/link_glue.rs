//! Daemon side of the Controller link.
//!
//! * Controller role: a listener manager keeps the TCP listener(s) in line with the configuration
//!   (role, bind address, port) and answers `sync` requests as the forwarding centre: it records
//!   AP admission, stores the LAN report of each Gateway and hands the profile and the Gateway
//!   LAN information to admitted APs.
//! * AP / Gateway roles: a client loop keeps one session to the Controller and syncs every
//!   [`SYNC_INTERVAL`]. A node on the Controller's own device reaches it over loopback and needs
//!   no account.
//!
//! The link only exchanges information. It never changes the local network by itself: the AP
//! still applies through the write plan, behind admission, apply-guard and the other rails.

use super::*;
use crate::accounts::AccountKey;
use crate::link::{self, Backend, Credentials, Peer, ServerHandle, ServerOptions, Session};
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use wifisync_core::link::{
    check_bind, check_password, check_port, endpoint_with_port, valid_username, GatewayLan,
    NodeInfo, NodeRole, Response, SyncRequest, SyncResponse, SyncSecret, DEFAULT_PORT,
};

/// Seconds between two syncs of a node.
const SYNC_INTERVAL: Duration = Duration::from_secs(15);
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(60);
const LISTEN_RETRY: Duration = Duration::from_secs(10);
const MAX_GATEWAYS: usize = 16;
/// Devices that may wait for admission at once, and the registry size overall (every new device
/// is a write to flash, and the registry is shown to the administrator).
const MAX_PENDING: usize = 64;
const MAX_ADMISSIONS: usize = 512;
/// A Gateway that has not reported for this long is forgotten.
const LAN_STALE_SECS: i64 = 24 * 3600;
/// Secret store reference of the password used to log in to a remote Controller.
const PASSWORD_REF: &str = "controller_password";

#[derive(Default)]
pub(super) struct LinkState {
    servers: Vec<ServerHandle>,
    spec: Vec<SocketAddr>,
    listen_error: Option<String>,
    last_attempt: Option<Instant>,
    client: ClientStatus,
}

#[derive(Debug, Clone, Default, Serialize)]
struct ClientStatus {
    /// `idle`, `not_configured`, `connecting`, `connected` or `error`.
    state: &'static str,
    endpoint: Option<String>,
    auth: Option<&'static str>,
    last_ok: Option<i64>,
    last_error: Option<String>,
    admission: Option<AdmissionState>,
    profile_version: u64,
}

impl Daemon {
    /// Start the listener manager and the client loop (both stop on shutdown).
    pub(super) fn spawn_link_threads(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        std::thread::spawn(move || {
            while !signals::shutdown_requested() {
                manager.reconcile_listeners();
                signals::sleep_interruptible(1_000);
            }
            manager.stop_listeners();
        });

        let client = Arc::clone(self);
        std::thread::spawn(move || client.client_loop());
    }

    /// Ask the client loop to re-read the configuration now.
    pub(super) fn kick_link(&self) {
        self.link_kick.store(true, Ordering::Relaxed);
    }

    // ── Controller: listeners ─────────────────────────────────────────────────

    /// Addresses the Controller should listen on: the configured one, plus loopback when a
    /// specific LAN address is configured, so that same-device nodes keep the login-free path.
    fn desired_listeners(cfg: &WifisyncConfig) -> Vec<SocketAddr> {
        if !cfg.roles.controller {
            return Vec::new();
        }
        let Ok(ip) = cfg.controller_bind.parse::<IpAddr>() else {
            return Vec::new();
        };
        let mut addrs = vec![SocketAddr::new(ip, cfg.controller_port)];
        if !ip.is_unspecified() && !wifisync_core::link::is_loopback(ip) {
            addrs.push(SocketAddr::new(
                IpAddr::from([127, 0, 0, 1]),
                cfg.controller_port,
            ));
        }
        addrs
    }

    fn reconcile_listeners(self: &Arc<Self>) {
        let desired = Self::desired_listeners(&self.config_snapshot());
        let mut link = self.link.lock().unwrap();

        if link.spec != desired {
            for server in link.servers.drain(..) {
                log_info!("link: stopped listening on {}", server.addr());
                server.stop();
            }
            link.spec = desired.clone();
            link.listen_error = None;
            link.last_attempt = None;
            // Let the old accept loops release their sockets before binding again
            std::thread::sleep(Duration::from_millis(250));
        }
        if desired.is_empty() || !link.servers.is_empty() {
            return;
        }
        if link
            .last_attempt
            .is_some_and(|t| t.elapsed() < LISTEN_RETRY)
        {
            return;
        }
        link.last_attempt = Some(Instant::now());

        let backend: Arc<dyn Backend> = self.clone();
        let mut started = Vec::new();
        for addr in desired {
            match link::start_server(addr, backend.clone(), ServerOptions::default()) {
                Ok(handle) => started.push(handle),
                Err(e) => {
                    // All or nothing: a half-started Controller would be confusing
                    for handle in &started {
                        handle.stop();
                    }
                    if link.listen_error.as_deref() != Some(e.as_str()) {
                        log_error!("link: {}", e);
                    }
                    link.listen_error = Some(e);
                    return;
                }
            }
        }
        for handle in &started {
            log_info!("link: Controller listening on {}", handle.addr());
        }
        link.listen_error = None;
        link.servers = started;
    }

    fn stop_listeners(&self) {
        let mut link = self.link.lock().unwrap();
        for server in link.servers.drain(..) {
            server.stop();
        }
        link.spec.clear();
    }

    // ── Controller: serving a sync ────────────────────────────────────────────

    fn controller_sync(&self, peer: &Peer, request: SyncRequest) -> Result<SyncResponse, String> {
        let node = &request.node;
        let mut response = SyncResponse {
            server_time: now(),
            ..SyncResponse::default()
        };

        let mut admission = None;
        if node.has_role(NodeRole::Ap) {
            let state = self.admit_node(peer, node)?;
            admission = Some(state);
            response.admission = admission;
        }

        if node.has_role(NodeRole::Gateway) {
            if let Some(report) = request.lan {
                let mut lans = self.gateway_lans.lock().unwrap();
                lans.retain(|_, entry| now() - entry.reported_at < LAN_STALE_SECS);
                if !lans.contains_key(&node.device_id) && lans.len() >= MAX_GATEWAYS {
                    return Err("too many Gateways have reported to this Controller".to_string());
                }
                lans.insert(
                    node.device_id.clone(),
                    GatewayLan {
                        device_id: node.device_id.clone(),
                        hostname: node.hostname.clone(),
                        source_addr: Some(peer.addr.to_string()),
                        reported_at: now(),
                        report,
                    },
                );
            }
        }

        // Forwarding: only an admitted AP receives the profile and the Gateway information
        if admission == Some(AdmissionState::Approved) {
            let profile = self.profile.lock().unwrap().clone();
            if profile.version > request.profile_version && !profile.is_empty() {
                // The key travels *beside* the profile, never inside it: the profile is published
                // through LuCI. Sent only on a (re)distribution, so a node does not rewrite flash
                // on every sync.
                response.secrets = self.secrets_for(&profile);
                response.profile = Some(profile);
            }
            response.lan = self
                .gateway_lans
                .lock()
                .unwrap()
                .values()
                .cloned()
                .collect();
        }
        Ok(response)
    }

    /// Secrets that must accompany a profile: today the Wi-Fi key of every encrypted radio.
    ///
    /// Only the references the profile actually uses are shipped (least privilege), and only when
    /// the value passes the shared key rules — an unusable key is never sent, and the AP would
    /// refuse to apply it anyway. Values are never logged.
    fn secrets_for(&self, profile: &NetworkProfile) -> Vec<SyncSecret> {
        let mut out = Vec::new();
        for wifi in &profile.wifi {
            if !wifisync_core::wifi_key::requires_key(&wifi.auth) {
                continue;
            }
            if !wifisync_core::link::valid_secret_reference(&wifi.psk_ref) {
                log_warn!("link: refusing to ship a secret with an invalid reference");
                continue;
            }
            let Some(value) = self.resolve_secret(&wifi.psk_ref) else {
                log_warn!(
                    "link: no Wi-Fi key available for reference `{}`; the AP will refuse to apply",
                    wifi.psk_ref
                );
                continue;
            };
            if wifisync_core::wifi_key::check(&wifi.auth, Some(&value)).is_err() {
                log_warn!(
                    "link: the Wi-Fi key for reference `{}` is unusable; not shipping it",
                    wifi.psk_ref
                );
                continue;
            }
            out.push(SyncSecret {
                reference: wifi.psk_ref.clone(),
                value,
            });
        }
        out
    }

    /// Where the secret for `reference` comes from: the local secret store first (provisioned with
    /// `wifisync secret set`, and what the `custom` Wi-Fi source uses), then this device's own
    /// `wireless` configuration (the `controller_self` source).
    ///
    /// Never returns the value in a log or an error message.
    pub(super) fn resolve_secret(&self, reference: &str) -> Option<String> {
        if let Some(value) = self.secrets.get(reference).ok().flatten() {
            return Some(value);
        }
        if reference == wifisync_sys::sysfs::CONTROLLER_PSK_REF {
            let text = self.uci.read_config_file("wireless").ok().flatten()?;
            return wifisync_sys::sysfs::own_wifi_key(&text);
        }
        None
    }

    /// Record an AP in the admission registry. Persists only on a real change: a node syncs
    /// every few seconds and `/etc` is flash.
    fn admit_node(&self, peer: &Peer, node: &NodeInfo) -> Result<AdmissionState, String> {
        let mut entry = AdmissionEntry::new(&node.device_id, &node.mac, now());
        entry.hostname = node.hostname.clone();
        entry.model = node.model.clone();
        entry.radios = node.radios;
        entry.source_addr = Some(peer.addr.to_string());

        let (state, changed, snapshot) = {
            let mut registry = self.admissions.lock().unwrap();
            let known = registry.find(&node.device_id).is_some();
            if !known
                && !peer.loopback
                && (registry.pending_count() >= MAX_PENDING
                    || registry.entries().len() >= MAX_ADMISSIONS)
            {
                return Err(
                    "too many devices wait for admission: approve or reject some first".to_string(),
                );
            }
            let mut state = registry.register(entry);
            let mut changed = !known;
            // Same device as the Controller: no verification step
            if peer.loopback && state == AdmissionState::Pending {
                registry
                    .approve(&node.device_id, now())
                    .map_err(|e| e.to_string())?;
                state = AdmissionState::Approved;
                changed = true;
            }
            (state, changed, registry.clone())
        };
        if changed {
            self.state
                .save_admissions(&snapshot)
                .map_err(rpc::sys_err)?;
        }
        Ok(state)
    }

    // ── AP / Gateway: client loop ─────────────────────────────────────────────

    /// Where this device finds its Controller.
    fn link_endpoint(cfg: &WifisyncConfig) -> Option<String> {
        match &cfg.controller_endpoint {
            Some(endpoint) => Some(endpoint_with_port(endpoint, DEFAULT_PORT)),
            None if cfg.roles.controller => Some(format!("127.0.0.1:{}", cfg.controller_port)),
            None => None,
        }
    }

    fn link_credentials(&self, cfg: &WifisyncConfig) -> Option<Credentials> {
        let username = cfg.controller_username.clone()?;
        let password = self.secrets.get(PASSWORD_REF).ok().flatten()?;
        Some(Credentials { username, password })
    }

    fn set_client_status(&self, update: impl FnOnce(&mut ClientStatus)) {
        update(&mut self.link.lock().unwrap().client);
    }

    /// Wait up to `delay`, waking early on shutdown or a configuration change.
    fn link_sleep(&self, delay: Duration) {
        let end = Instant::now() + delay;
        while Instant::now() < end && !signals::shutdown_requested() {
            if self.link_kick.swap(false, Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn client_loop(self: Arc<Self>) {
        type Key = (String, Option<(String, String)>);
        let mut session: Option<(Session, Key)> = None;
        let mut retry = RETRY_MIN;
        let mut last_error = String::new();

        while !signals::shutdown_requested() {
            let cfg = self.config_snapshot();
            if !cfg.roles.ap && !cfg.roles.gateway {
                session = None;
                self.set_client_status(|c| {
                    *c = ClientStatus {
                        state: "idle",
                        ..Default::default()
                    }
                });
                self.link_sleep(Duration::from_secs(2));
                continue;
            }
            let Some(endpoint) = Self::link_endpoint(&cfg) else {
                session = None;
                self.set_client_status(|c| {
                    *c = ClientStatus {
                        state: "not_configured",
                        ..Default::default()
                    }
                });
                self.link_sleep(Duration::from_secs(2));
                continue;
            };
            let credentials = self.link_credentials(&cfg);
            let key: Key = (
                endpoint.clone(),
                credentials
                    .as_ref()
                    .map(|c| (c.username.clone(), c.password.clone())),
            );
            if session.as_ref().is_some_and(|(_, current)| *current != key) {
                session = None;
            }

            let outcome = (|| -> Result<(SyncResponse, &'static str), String> {
                if session.is_none() {
                    self.set_client_status(|c| {
                        c.state = "connecting";
                        c.endpoint = Some(endpoint.clone());
                    });
                    let opened = Session::connect(&endpoint, &cfg.device_id, credentials.as_ref())?;
                    session = Some((opened, key.clone()));
                }
                let (open, _) = session.as_mut().expect("session was just opened");
                let auth = open.auth.as_str();
                let request = self.build_sync_request(&cfg);
                Ok((open.sync(&request)?, auth))
            })();

            let delay = match outcome {
                Ok((response, auth)) => {
                    if !last_error.is_empty() {
                        log_info!("link: connected to the Controller at {}", endpoint);
                        last_error.clear();
                    }
                    retry = RETRY_MIN;
                    self.apply_sync_response(&cfg, &response);
                    let version = self.profile.lock().unwrap().version;
                    self.set_client_status(|c| {
                        *c = ClientStatus {
                            state: "connected",
                            endpoint: Some(endpoint.clone()),
                            auth: Some(auth),
                            last_ok: Some(now()),
                            last_error: None,
                            admission: response.admission,
                            profile_version: version,
                        }
                    });
                    SYNC_INTERVAL
                }
                Err(e) => {
                    session = None;
                    if last_error != e {
                        log_warn!("link: Controller {} unavailable: {}", endpoint, e);
                        last_error = e.clone();
                    }
                    self.set_client_status(|c| {
                        c.state = "error";
                        c.endpoint = Some(endpoint.clone());
                        c.auth = None;
                        c.last_error = Some(e);
                    });
                    let delay = retry;
                    retry = (retry * 2).min(RETRY_MAX);
                    delay
                }
            };
            self.link_sleep(delay);
        }
    }

    fn local_mac(&self, cfg: &WifisyncConfig) -> String {
        let mut candidates = vec![cfg.bridge_name.clone()];
        candidates.extend(self.caps_snapshot().port_names());
        candidates
            .iter()
            .filter(|name| wifisync_core::link::valid_identifier(name, 15))
            .find_map(|name| {
                std::fs::read_to_string(
                    self.paths
                        .root()
                        .join("sys/class/net")
                        .join(name)
                        .join("address"),
                )
                .ok()
            })
            .map(|mac| mac.trim().to_string())
            .unwrap_or_default()
    }

    fn local_node_info(&self, cfg: &WifisyncConfig) -> NodeInfo {
        let mut roles = Vec::new();
        if cfg.roles.ap {
            roles.push(NodeRole::Ap);
        }
        if cfg.roles.gateway {
            roles.push(NodeRole::Gateway);
        }
        let hostname = std::fs::read_to_string(self.paths.root().join("proc/sys/kernel/hostname"))
            .ok()
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty());
        let model = wifisync_sys::sysfs::read_board_json(&self.paths).and_then(|board| {
            board
                .pointer("/model/name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
        NodeInfo {
            device_id: cfg.device_id.clone(),
            roles,
            mac: self.local_mac(cfg),
            hostname,
            model,
            radios: self.caps_snapshot().radios.len() as u32,
        }
    }

    fn build_sync_request(&self, cfg: &WifisyncConfig) -> SyncRequest {
        SyncRequest {
            node: self.local_node_info(cfg),
            profile_version: self.profile.lock().unwrap().version,
            lan: cfg.roles.gateway.then(|| self.collect_lan(cfg)),
        }
    }

    pub(super) fn collect_lan(&self, cfg: &WifisyncConfig) -> wifisync_core::lan::LanReport {
        let mut report = wifisync_sys::lan::collect(&self.paths, &cfg.bridge_name, now());
        // The Controller rejects oversized reports, so an unusual device must still fit
        report.clamp();
        report
    }

    /// Take over what the Controller answered. On the Controller's own device the registry and
    /// the profile already are the Controller's, so there is nothing to mirror.
    fn apply_sync_response(&self, cfg: &WifisyncConfig, response: &SyncResponse) {
        if cfg.roles.controller {
            return;
        }
        if cfg.roles.ap {
            if let Some(state) = response.admission {
                if let Err(e) = self.mirror_admission(cfg, state) {
                    log_warn!("link: recording the admission state failed: {}", e);
                }
            }
            if let Some(profile) = &response.profile {
                let mut current = self.profile.lock().unwrap();
                if profile.validate().is_ok() {
                    *current = current.merge(profile);
                }
            }
            // Store the secrets that came with the profile (0600, root only). A failure is
            // reported by reference only: the value never reaches a log line.
            for secret in &response.secrets {
                if !wifisync_core::link::valid_secret_reference(&secret.reference) {
                    log_warn!("link: ignoring a secret with an invalid reference");
                    continue;
                }
                if let Err(e) = self.secrets.put(&secret.reference, &secret.value) {
                    log_warn!(
                        "link: storing the secret for reference `{}` failed: {}",
                        secret.reference,
                        e
                    );
                }
            }
            *self.remote_lans.lock().unwrap() = response.lan.clone();
        }
    }

    /// An AP applies only when its own registry says "approved" (`apply()` checks it), so the
    /// Controller's decision is mirrored there.
    fn mirror_admission(&self, cfg: &WifisyncConfig, state: AdmissionState) -> Result<(), String> {
        let snapshot = {
            let mut registry = self.admissions.lock().unwrap();
            let before = registry.find(&cfg.device_id).map(|e| e.state);
            if before == Some(state) {
                return Ok(());
            }
            if before.is_none() {
                registry.register(AdmissionEntry::new(&cfg.device_id, "", now()));
            }
            match state {
                AdmissionState::Approved => registry.approve(&cfg.device_id, now()),
                AdmissionState::Rejected => registry.reject(&cfg.device_id, now()),
                AdmissionState::Pending => registry.revoke(&cfg.device_id, now()),
            }
            .map_err(|e| e.to_string())?;
            registry.clone()
        };
        self.state.save_admissions(&snapshot).map_err(rpc::sys_err)
    }

    // ── RPC ───────────────────────────────────────────────────────────────────

    pub(super) fn link_status(&self) -> Value {
        let cfg = self.config_snapshot();
        let link = self.link.lock().unwrap();
        json!({
            "controller": {
                "enabled": cfg.roles.controller,
                "bind": cfg.controller_bind,
                "port": cfg.controller_port,
                "listening": link.servers.iter().map(|s| s.addr().to_string()).collect::<Vec<_>>(),
                "error": link.listen_error,
                "sessions": link.servers.iter().map(|s| s.active()).sum::<usize>(),
                "accounts": self.accounts.list().map(|a| a.len()).unwrap_or(0),
                "gateways": self.gateway_lans.lock().unwrap().len(),
            },
            "client": {
                "enabled": cfg.roles.ap || cfg.roles.gateway,
                "status": link.client,
            },
        })
    }

    pub(super) fn rpc_link_get(&self) -> Result<Value, String> {
        let cfg = self.config_snapshot();
        Ok(json!({
            "controller_endpoint": cfg.controller_endpoint,
            "effective_endpoint": Self::link_endpoint(&cfg),
            "controller_username": cfg.controller_username,
            "password_set": self.secrets.contains(PASSWORD_REF),
            "controller_port": cfg.controller_port,
            "controller_bind": cfg.controller_bind,
            "default_port": DEFAULT_PORT,
            "loopback_login_free": true,
        }))
    }

    pub(super) fn rpc_link_set(&self, params: &Value) -> Result<Value, String> {
        let text = |name: &str| params.get(name).and_then(|v| v.as_str());
        let mut next = self.config_snapshot();

        if let Some(endpoint) = text("controller_endpoint") {
            next.controller_endpoint = Some(endpoint.trim().to_string()).filter(|e| !e.is_empty());
        }
        if let Some(username) = text("controller_username") {
            next.controller_username = Some(username.to_string()).filter(|u| !u.is_empty());
        }
        if let Some(port) = params.get("controller_port").and_then(|v| v.as_u64()) {
            next.controller_port = check_port(port)?;
        }
        if let Some(bind) = text("controller_bind") {
            check_bind(bind)?;
            next.controller_bind = bind.to_string();
        }
        if let Some(endpoint) = &next.controller_endpoint {
            if endpoint
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
            {
                return Err("the Controller address must not contain whitespace".to_string());
            }
        }
        if let Some(name) = &next.controller_username {
            if !valid_username(name) {
                return Err("the account name may only contain letters, digits and - _ . :".into());
            }
        }
        let password = text("controller_password").filter(|p| !p.is_empty());
        if let Some(password) = password {
            check_password(password)?;
        }
        next.validate(&self.caps_snapshot())
            .map_err(|e| e.to_string())?;

        if let Some(password) = password {
            self.secrets
                .put(PASSWORD_REF, password)
                .map_err(rpc::sys_err)?;
        }
        if params.get("clear_password").and_then(|v| v.as_bool()) == Some(true) {
            self.secrets.remove(PASSWORD_REF).map_err(rpc::sys_err)?;
        }
        *self.config.write().unwrap() = next;
        self.persist_config()?;
        self.kick_link();
        self.rpc_link_get()
    }

    fn require_controller(&self) -> Result<(), String> {
        if self.config_snapshot().roles.controller {
            Ok(())
        } else {
            Err("this device does not take the Controller role and has no accounts".to_string())
        }
    }

    pub(super) fn rpc_account(&self, method: &str, params: &Value) -> Result<Value, String> {
        let text = |name: &str| {
            params
                .get(name)
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("missing {} argument", name))
        };
        match method {
            "account.list" => Ok(json!({ "accounts": self.accounts.list()? })),
            "account.add" => {
                self.require_controller()?;
                self.accounts
                    .add(text("username")?, text("password")?, now())?;
                Ok(json!({ "added": text("username")? }))
            }
            "account.passwd" => {
                self.require_controller()?;
                self.accounts
                    .set_password(text("username")?, text("password")?)?;
                Ok(json!({ "updated": text("username")? }))
            }
            "account.remove" => {
                self.require_controller()?;
                self.accounts.remove(text("username")?)?;
                Ok(json!({ "removed": text("username")? }))
            }
            other => Err(format!("unknown method `{}`", other)),
        }
    }

    /// The LAN reports the Controller holds, or (on an AP) the ones the Controller forwarded.
    pub(super) fn rpc_lan_list(&self) -> Result<Value, String> {
        let cfg = self.config_snapshot();
        let (source, entries): (&str, Vec<GatewayLan>) = if cfg.roles.controller {
            (
                "controller",
                self.gateway_lans
                    .lock()
                    .unwrap()
                    .values()
                    .cloned()
                    .collect(),
            )
        } else {
            ("forwarded", self.remote_lans.lock().unwrap().clone())
        };
        Ok(json!({ "source": source, "entries": entries, "now": now() }))
    }
}

impl Backend for Daemon {
    fn serving(&self) -> bool {
        self.config.read().unwrap().roles.controller
    }

    fn account(&self, username: &str) -> Option<AccountKey> {
        self.accounts.lookup(username)
    }

    fn sync(&self, peer: &Peer, request: SyncRequest) -> Response {
        match self.controller_sync(peer, request) {
            Ok(response) => Response::Sync(response),
            Err(message) => Response::Error { message },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wifisync_core::lan::LanReport;

    fn daemon(tag: &str, roles: &str) -> (Daemon, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("wifisync-glue-{}", tag));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("etc/config")).unwrap();
        std::fs::create_dir_all(root.join("sys/class/ieee80211/phy0")).unwrap();
        std::fs::write(
            root.join("etc/config/wifisync"),
            format!(
                "config wifisync 'main'\n\toption roles '{}'\n\toption roles_configured '1'\n",
                roles
            ),
        )
        .unwrap();
        let daemon = Daemon::new(Paths::with_root(&root), DaemonOptions::default()).unwrap();
        (daemon, root)
    }

    fn peer(loopback: bool) -> Peer {
        Peer {
            addr: if loopback {
                IpAddr::from([127, 0, 0, 1])
            } else {
                IpAddr::from([192, 168, 1, 50])
            },
            loopback,
            username: (!loopback).then(|| "ap1".to_string()),
        }
    }

    fn ap(id: &str) -> SyncRequest {
        SyncRequest {
            node: NodeInfo {
                device_id: id.into(),
                roles: vec![NodeRole::Ap],
                ..NodeInfo::default()
            },
            ..SyncRequest::default()
        }
    }

    fn gateway(id: &str) -> SyncRequest {
        SyncRequest {
            node: NodeInfo {
                device_id: id.into(),
                roles: vec![NodeRole::Gateway],
                ..NodeInfo::default()
            },
            lan: Some(LanReport::default()),
            ..SyncRequest::default()
        }
    }

    #[test]
    fn a_loopback_ap_is_admitted_without_verification_a_remote_one_is_not() {
        let (daemon, root) = daemon("admit", "controller ap");
        let local = daemon.controller_sync(&peer(true), ap("local")).unwrap();
        assert_eq!(local.admission, Some(AdmissionState::Approved));
        let remote = daemon.controller_sync(&peer(false), ap("remote")).unwrap();
        assert_eq!(remote.admission, Some(AdmissionState::Pending));

        // An explicit rejection also holds against a loopback node
        daemon
            .admissions
            .lock()
            .unwrap()
            .reject("local", now())
            .unwrap();
        let again = daemon.controller_sync(&peer(true), ap("local")).unwrap();
        assert_eq!(again.admission, Some(AdmissionState::Rejected));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn routine_syncs_do_not_rewrite_the_state_file() {
        let (daemon, root) = daemon("flash", "controller ap");
        daemon.controller_sync(&peer(false), ap("remote")).unwrap();
        let state = daemon.paths.state_file();
        assert!(state.exists(), "a new AP is persisted");
        std::fs::remove_file(&state).unwrap();
        for _ in 0..3 {
            daemon.controller_sync(&peer(false), ap("remote")).unwrap();
        }
        assert!(!state.exists(), "an unchanged AP must not touch flash");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn only_an_admitted_ap_receives_the_profile_and_the_gateway_lan() {
        let (daemon, root) = daemon("forward", "controller ap gateway");
        let mut profile = NetworkProfile::default();
        profile.version = 5;
        profile
            .wifi
            .push(wifisync_core::profile::WifiProfile::template("radio0"));
        *daemon.profile.lock().unwrap() = profile;

        daemon
            .controller_sync(&peer(false), gateway("gw1"))
            .unwrap();

        let pending = daemon.controller_sync(&peer(false), ap("ap1")).unwrap();
        assert!(pending.profile.is_none() && pending.lan.is_empty());

        daemon
            .admissions
            .lock()
            .unwrap()
            .approve("ap1", now())
            .unwrap();
        let approved = daemon.controller_sync(&peer(false), ap("ap1")).unwrap();
        assert_eq!(approved.profile.as_ref().map(|p| p.version), Some(5));
        assert_eq!(approved.lan.len(), 1);
        assert_eq!(approved.lan[0].device_id, "gw1");
        assert_eq!(approved.lan[0].source_addr.as_deref(), Some("192.168.1.50"));

        // Up to date: the profile is not sent again
        let mut current = ap("ap1");
        current.profile_version = 5;
        let up_to_date = daemon.controller_sync(&peer(false), current).unwrap();
        assert!(up_to_date.profile.is_none());

        // A Gateway is not an AP: it gets nothing forwarded
        let gw = daemon
            .controller_sync(&peer(false), gateway("gw1"))
            .unwrap();
        assert!(gw.admission.is_none() && gw.profile.is_none() && gw.lan.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_number_of_reporting_gateways_is_bounded() {
        let (daemon, root) = daemon("cap", "controller");
        for i in 0..MAX_GATEWAYS {
            daemon
                .controller_sync(&peer(false), gateway(&format!("gw{}", i)))
                .unwrap();
        }
        assert!(daemon
            .controller_sync(&peer(false), gateway("one-too-many"))
            .is_err());
        // A known Gateway can still refresh its report
        assert!(daemon.controller_sync(&peer(false), gateway("gw0")).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_admission_registry_cannot_be_flooded() {
        let (daemon, root) = daemon("flood", "controller");
        for i in 0..MAX_PENDING {
            daemon
                .controller_sync(&peer(false), ap(&format!("ap{}", i)))
                .unwrap();
        }
        let error = daemon
            .controller_sync(&peer(false), ap("one-too-many"))
            .unwrap_err();
        assert!(error.contains("too many"), "{}", error);
        // Known devices keep syncing, and a node on the Controller's own device is not blocked
        assert!(daemon.controller_sync(&peer(false), ap("ap0")).is_ok());
        assert_eq!(
            daemon
                .controller_sync(&peer(true), ap("local"))
                .unwrap()
                .admission,
            Some(AdmissionState::Approved)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_gateways_are_forgotten() {
        let (daemon, root) = daemon("stale", "controller");
        daemon
            .controller_sync(&peer(false), gateway("old"))
            .unwrap();
        daemon
            .gateway_lans
            .lock()
            .unwrap()
            .get_mut("old")
            .unwrap()
            .reported_at = now() - LAN_STALE_SECS - 1;
        daemon
            .controller_sync(&peer(false), gateway("new"))
            .unwrap();
        let lans = daemon.gateway_lans.lock().unwrap();
        assert!(!lans.contains_key("old") && lans.contains_key("new"));
        drop(lans);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_ap_mirrors_the_controllers_decision() {
        let (daemon, root) = daemon("mirror", "ap");
        let cfg = daemon.config_snapshot();
        let approved = |daemon: &Daemon| {
            daemon
                .admissions
                .lock()
                .unwrap()
                .require_approved(&cfg.device_id)
                .is_ok()
        };
        assert!(!approved(&daemon));

        let mut response = SyncResponse {
            admission: Some(AdmissionState::Approved),
            ..SyncResponse::default()
        };
        daemon.apply_sync_response(&cfg, &response);
        assert!(approved(&daemon));

        response.admission = Some(AdmissionState::Rejected);
        daemon.apply_sync_response(&cfg, &response);
        assert!(!approved(&daemon));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn listeners_follow_role_bind_and_port() {
        let mut cfg = WifisyncConfig::default();
        cfg.roles = Roles {
            controller: false,
            ap: true,
            gateway: false,
        };
        assert!(Daemon::desired_listeners(&cfg).is_empty());

        cfg.roles.controller = true;
        cfg.controller_port = 7000;
        let all = Daemon::desired_listeners(&cfg);
        assert_eq!(all, vec!["0.0.0.0:7000".parse().unwrap()]);

        // A specific LAN address keeps a loopback listener for same-device nodes
        cfg.controller_bind = "192.168.1.1".into();
        let specific = Daemon::desired_listeners(&cfg);
        assert_eq!(
            specific,
            vec![
                "192.168.1.1:7000".parse().unwrap(),
                "127.0.0.1:7000".parse().unwrap()
            ]
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn the_node_finds_its_controller() {
        let mut cfg = WifisyncConfig::default();
        cfg.roles = Roles {
            controller: false,
            ap: true,
            gateway: false,
        };
        assert_eq!(Daemon::link_endpoint(&cfg), None);
        cfg.controller_endpoint = Some("10.0.0.1".into());
        assert_eq!(
            Daemon::link_endpoint(&cfg).as_deref(),
            Some("10.0.0.1:6550")
        );
        cfg.controller_endpoint = None;
        cfg.roles.controller = true;
        cfg.controller_port = 7000;
        assert_eq!(
            Daemon::link_endpoint(&cfg).as_deref(),
            Some("127.0.0.1:7000")
        );
    }

    #[test]
    fn link_settings_are_validated_and_the_password_stays_secret() {
        let (daemon, root) = daemon("settings", "ap");
        assert!(daemon
            .rpc_link_set(&json!({ "controller_port": 0 }))
            .is_err());
        assert!(daemon
            .rpc_link_set(&json!({ "controller_bind": "lan" }))
            .is_err());
        assert!(daemon
            .rpc_link_set(&json!({ "controller_username": "a b" }))
            .is_err());
        assert!(daemon
            .rpc_link_set(&json!({ "controller_password": "short" }))
            .is_err());
        assert!(!daemon.secrets.contains(PASSWORD_REF));

        // No uci on a host: the settings still apply, only persisting reports an error
        let _ = daemon.rpc_link_set(&json!({
            "controller_endpoint": "10.0.0.1",
            "controller_username": "ap1",
            "controller_password": "correct horse",
        }));
        let got = daemon.rpc_link_get().unwrap();
        assert_eq!(got["effective_endpoint"], "10.0.0.1:6550");
        assert_eq!(got["password_set"], true);
        assert!(!got.to_string().contains("correct horse"));
        assert!(!daemon.status().to_string().contains("correct horse"));
        let _ = std::fs::remove_dir_all(root);
    }
}
