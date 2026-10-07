//! Controller link: the wire protocol between the Controller and the AP / Gateway nodes.
//!
//! * Transport: TCP, one JSON object per line (same framing as the local UNIX socket API).
//! * No TLS (project constraint, see `secrets.rs`). Instead:
//!   1. the node proves it knows the account password with an HMAC challenge-response over a
//!      PBKDF2-derived key, and the Controller proves it back (mutual authentication);
//!   2. after that every frame is sealed with ChaCha20-Poly1305 under a per-session key, with the
//!      direction and a strictly increasing sequence number as associated data (no replay, no
//!      reordering).
//! * **Loopback exemption**: when the peer address is a loopback address the node and the
//!   Controller share the device, so authentication and encryption are skipped. A client refuses
//!   such an answer from a non-loopback peer (no downgrade by a man in the middle).
//!
//! Handshake (plain JSON lines):
//!
//! ```text
//! C -> S  {"t":"hello","v":1,"device_id":"..","username":".."}
//! S -> C  {"t":"ready","auth":"loopback"}                      (loopback peers, done)
//! S -> C  {"t":"challenge","salt":hex,"iter":n,"nonce":hex}
//! C -> S  {"t":"auth","cnonce":hex,"proof":hex}
//! S -> C  {"t":"ready","auth":"password","proof":hex}          (or {"t":"denied"})
//! ```
//!
//! then application frames: `{"n":<seq>,"d":"<hex sealed JSON>"}` (plain JSON on loopback).

use crate::accounts::{derive_key, AccountKey, DEFAULT_ITERATIONS};
use crate::secrets::{constant_time_eq, hex, hmac_hex, open, random_bytes, seal, unhex};
use crate::{log_info, log_warn};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wifisync_core::link::{
    is_loopback, valid_identifier, valid_username, Request, Response, SyncRequest, SyncResponse,
    MAX_DEVICE_ID_LEN, PROTOCOL_VERSION,
};

/// Largest frame of an established session.
const MAX_LINE: usize = 512 * 1024;
/// Largest frame before the login has completed (a hello or an auth is a few hundred bytes).
const PRE_AUTH_LINE: usize = 4 * 1024;
/// A frame that has started must be complete within this time (slowloris guard).
const FRAME_TIMEOUT: Duration = Duration::from_secs(15);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Server read tick: how often a connection thread notices a shutdown or an idle timeout.
const TICK: Duration = Duration::from_secs(1);
const MAX_CONNECTIONS: usize = 32;
const MAX_PER_IP: usize = 8;
/// Delay before a failed login is answered (slows online guessing down).
const DENY_DELAY: Duration = Duration::from_secs(1);
const THREAD_STACK: usize = 256 * 1024;

const C2S: &[u8] = b"c2s";
const S2C: &[u8] = b"s2c";

// ── Framing ──────────────────────────────────────────────────────────────────

/// One side of a link connection: line framing plus the optional frame encryption.
struct Transport {
    stream: TcpStream,
    buf: Vec<u8>,
    max_line: usize,
    /// When the oldest incomplete frame in `buf` started.
    partial_since: Option<Instant>,
    /// Hard limit for the frame being waited for.
    deadline: Option<Instant>,
    stop: Option<Arc<AtomicBool>>,
    key: Option<Vec<u8>>,
    send_seq: u64,
    recv_seq: u64,
    send_dir: &'static [u8],
    recv_dir: &'static [u8],
}

fn aad(direction: &[u8], seq: u64) -> Vec<u8> {
    let mut aad = direction.to_vec();
    aad.extend_from_slice(&seq.to_le_bytes());
    aad
}

impl Transport {
    fn new(stream: TcpStream, send_dir: &'static [u8], recv_dir: &'static [u8]) -> Self {
        Self {
            stream,
            buf: Vec::new(),
            max_line: PRE_AUTH_LINE,
            partial_since: None,
            deadline: None,
            stop: None,
            key: None,
            send_seq: 0,
            recv_seq: 0,
            send_dir,
            recv_dir,
        }
    }

    /// Next complete line; `Ok(None)` when the read timed out (partial data is kept).
    ///
    /// Every limit is enforced inside the loop: a peer that drips one byte at a time never makes
    /// `read` time out, so the caller's own checks would not run.
    fn read_line(&mut self) -> Result<Option<Vec<u8>>, String> {
        loop {
            if let Some(end) = self.buf.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=end).collect();
                line.pop();
                self.partial_since = (!self.buf.is_empty()).then(Instant::now);
                return Ok(Some(line));
            }
            if self.buf.len() > self.max_line {
                return Err("frame too large".to_string());
            }
            if self
                .stop
                .as_ref()
                .is_some_and(|s| s.load(Ordering::Relaxed))
            {
                return Err("shutting down".to_string());
            }
            if self.deadline.is_some_and(|d| Instant::now() >= d) {
                return Err("timed out".to_string());
            }
            if !self.buf.is_empty()
                && self
                    .partial_since
                    .get_or_insert_with(Instant::now)
                    .elapsed()
                    > FRAME_TIMEOUT
            {
                return Err("frame timed out".to_string());
            }
            let mut chunk = [0u8; 4096];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err("connection closed".to_string()),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None)
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("read failed: {}", e)),
            }
        }
    }

    fn write_line(&mut self, value: &Value) -> Result<(), String> {
        let mut line = value.to_string();
        line.push('\n');
        self.stream
            .write_all(line.as_bytes())
            .and_then(|_| self.stream.flush())
            .map_err(|e| format!("write failed: {}", e))
    }

    /// Next frame, decrypted when a session key is set; `Ok(None)` on a read timeout.
    fn recv(&mut self) -> Result<Option<Value>, String> {
        let Some(line) = self.read_line()? else {
            return Ok(None);
        };
        let frame: Value =
            serde_json::from_slice(&line).map_err(|e| format!("invalid frame: {}", e))?;
        let Some(key) = self.key.clone() else {
            return Ok(Some(frame));
        };
        let seq = frame
            .get("n")
            .and_then(|v| v.as_u64())
            .ok_or("frame without a sequence number")?;
        if seq != self.recv_seq {
            return Err("frame out of sequence (replay or loss)".to_string());
        }
        let sealed = frame
            .get("d")
            .and_then(|v| v.as_str())
            .and_then(unhex)
            .ok_or("frame without a payload")?;
        let plain = open(&key, &sealed, &aad(self.recv_dir, seq)).map_err(|e| e.to_string())?;
        self.recv_seq += 1;
        serde_json::from_slice(&plain)
            .map(Some)
            .map_err(|e| format!("invalid payload: {}", e))
    }

    fn send(&mut self, value: &Value) -> Result<(), String> {
        let Some(key) = self.key.clone() else {
            return self.write_line(value);
        };
        let seq = self.send_seq;
        let plain = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        let sealed = seal(&key, &plain, &aad(self.send_dir, seq)).map_err(|e| e.to_string())?;
        self.send_seq += 1;
        self.write_line(&json!({ "n": seq, "d": hex(&sealed) }))
    }
}

// ── Handshake primitives ─────────────────────────────────────────────────────

/// Transcript every proof and the session key are bound to.
fn transcript(nonce: &[u8], cnonce: &[u8], device_id: &str) -> Vec<u8> {
    let mut data = nonce.to_vec();
    data.extend_from_slice(cnonce);
    data.extend_from_slice(device_id.as_bytes());
    data
}

fn labelled(label: &str, transcript: &[u8]) -> Vec<u8> {
    let mut data = label.as_bytes().to_vec();
    data.push(b'|');
    data.extend_from_slice(transcript);
    data
}

fn client_proof(key: &[u8; 32], transcript: &[u8]) -> String {
    hmac_hex(key, &labelled("wifisync-client", transcript))
}

fn server_proof(key: &[u8; 32], transcript: &[u8]) -> String {
    hmac_hex(key, &labelled("wifisync-server", transcript))
}

fn session_key(key: &[u8; 32], transcript: &[u8]) -> Vec<u8> {
    unhex(&hmac_hex(key, &labelled("wifisync-session", transcript))).unwrap_or_default()
}

// ── Server (Controller) ──────────────────────────────────────────────────────

/// A connected, identified peer.
#[derive(Debug, Clone)]
pub struct Peer {
    pub addr: IpAddr,
    pub loopback: bool,
    /// Account the peer logged in with (`None` for loopback peers).
    pub username: Option<String>,
}

/// What the Controller side needs from the daemon.
pub trait Backend: Send + Sync {
    /// Whether this device currently takes the Controller role.
    fn serving(&self) -> bool;
    fn account(&self, username: &str) -> Option<AccountKey>;
    fn sync(&self, peer: &Peer, request: SyncRequest) -> Response;
}

#[derive(Debug, Clone, Copy)]
pub struct ServerOptions {
    pub trust_loopback: bool,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            trust_loopback: true,
        }
    }
}

pub struct ServerHandle {
    stop: Arc<AtomicBool>,
    addr: SocketAddr,
    active: Arc<AtomicUsize>,
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    /// Close the listener and let the connection threads wind down.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Shared {
    backend: Arc<dyn Backend>,
    options: ServerOptions,
    stop: Arc<AtomicBool>,
    /// Open connections per source address.
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    /// Keys the decoy salts of unknown users, so that a missing account looks like an existing one.
    decoy_key: Vec<u8>,
}

/// Bind the listener and serve in background threads.
pub fn start_server(
    addr: SocketAddr,
    backend: Arc<dyn Backend>,
    options: ServerOptions,
) -> Result<ServerHandle, String> {
    let listener =
        TcpListener::bind(addr).map_err(|e| format!("cannot listen on {}: {}", addr, e))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("configuring the listener failed: {}", e))?;
    let local = listener.local_addr().unwrap_or(addr);

    let stop = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let shared = Arc::new(Shared {
        backend,
        options,
        stop: stop.clone(),
        per_ip: Mutex::new(HashMap::new()),
        decoy_key: random_bytes(32).map_err(|e| e.to_string())?,
    });

    let accept_active = active.clone();
    std::thread::Builder::new()
        .name("link-accept".into())
        .stack_size(THREAD_STACK)
        .spawn(move || accept_loop(listener, shared, accept_active))
        .map_err(|e| format!("starting the listener thread failed: {}", e))?;

    Ok(ServerHandle {
        stop,
        addr: local,
        active,
    })
}

/// A reserved connection slot, released when dropped (also when the thread cannot start).
struct Slot {
    shared: Arc<Shared>,
    active: Arc<AtomicUsize>,
    ip: IpAddr,
}

impl Slot {
    fn acquire(shared: &Arc<Shared>, active: &Arc<AtomicUsize>, ip: IpAddr) -> Option<Slot> {
        if active.fetch_add(1, Ordering::Relaxed) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        let mut per_ip = shared.per_ip.lock().unwrap();
        let count = per_ip.entry(ip).or_insert(0);
        if *count >= MAX_PER_IP {
            drop(per_ip);
            active.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        *count += 1;
        Some(Slot {
            shared: shared.clone(),
            active: active.clone(),
            ip,
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
        let mut per_ip = self.shared.per_ip.lock().unwrap();
        if let Some(count) = per_ip.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                per_ip.remove(&self.ip);
            }
        }
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, active: Arc<AtomicUsize>) {
    while !shared.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                let Some(slot) = Slot::acquire(&shared, &active, peer.ip().to_canonical()) else {
                    log_warn!("link: connection limit reached, dropping {}", peer.ip());
                    continue;
                };
                let shared = shared.clone();
                let spawned = std::thread::Builder::new()
                    .name("link-conn".into())
                    .stack_size(THREAD_STACK)
                    .spawn(move || {
                        if let Err(e) = serve_connection(stream, &shared) {
                            log_info!("link: connection ended: {}", e);
                        }
                        drop(slot);
                    });
                if spawned.is_err() {
                    log_warn!("link: cannot start a connection thread");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                log_warn!("link: accept failed: {}", e);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

/// Wait for the next frame until `deadline`, honouring a shutdown request.
fn recv_until(
    transport: &mut Transport,
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<Value, String> {
    transport.deadline = Some(deadline);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("shutting down".to_string());
        }
        if let Some(value) = transport.recv()? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err("timed out".to_string());
        }
    }
}

fn fail(transport: &mut Transport, message: &str) -> Result<(), String> {
    let _ = transport.send(&json!({ "t": "error", "message": message }));
    Err(message.to_string())
}

fn serve_connection(stream: TcpStream, shared: &Shared) -> Result<(), String> {
    stream
        .set_nonblocking(false)
        .and_then(|_| stream.set_read_timeout(Some(TICK)))
        .and_then(|_| stream.set_write_timeout(Some(CLIENT_TIMEOUT)))
        .map_err(|e| format!("configuring the socket failed: {}", e))?;
    let _ = stream.set_nodelay(true);
    let addr = stream
        .peer_addr()
        .map_err(|e| format!("no peer address: {}", e))?
        .ip();
    let mut transport = Transport::new(stream, S2C, C2S);
    transport.stop = Some(shared.stop.clone());
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;

    let hello = recv_until(&mut transport, deadline, &shared.stop)?;
    if hello.get("t").and_then(|v| v.as_str()) != Some("hello") {
        return fail(&mut transport, "expected hello");
    }
    if hello.get("v").and_then(|v| v.as_u64()) != Some(u64::from(PROTOCOL_VERSION)) {
        return fail(&mut transport, "unsupported protocol version");
    }
    let device_id = hello
        .get("device_id")
        .and_then(|v| v.as_str())
        .filter(|id| valid_identifier(id, MAX_DEVICE_ID_LEN))
        .ok_or("invalid device_id")?
        .to_string();
    if !shared.backend.serving() {
        return fail(&mut transport, "this device is not a Controller");
    }

    let loopback = shared.options.trust_loopback && is_loopback(addr);
    let mut peer = Peer {
        addr,
        loopback,
        username: None,
    };

    // The key the session was opened with: a removed or re-keyed account ends the session
    let mut login: Option<(String, [u8; 32])> = None;
    if loopback {
        transport.send(&json!({ "t": "ready", "auth": "loopback" }))?;
    } else {
        let (username, key) = authenticate(&mut transport, shared, &hello, &device_id, deadline)?;
        peer.username = Some(username.clone());
        login = Some((username, key));
    }
    transport.deadline = None;
    transport.max_line = MAX_LINE;

    let mut idle_deadline = Instant::now() + IDLE_TIMEOUT;
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let frame = match transport.recv()? {
            Some(frame) => frame,
            None if Instant::now() >= idle_deadline => return Err("idle timeout".to_string()),
            None => continue,
        };
        idle_deadline = Instant::now() + IDLE_TIMEOUT;

        if let Some((username, key)) = &login {
            match shared.backend.account(username) {
                Some(account) if account.key == *key => {}
                _ => return Err("the account was removed or its password changed".to_string()),
            }
        }

        let response = match serde_json::from_value::<Request>(frame) {
            Ok(Request::Sync(request)) => match request.validate() {
                Ok(()) if request.node.device_id != device_id => Response::Error {
                    message: "device_id differs from the one of the session".to_string(),
                },
                Ok(()) => shared.backend.sync(&peer, request),
                Err(message) => Response::Error { message },
            },
            Err(e) => Response::Error {
                message: format!("invalid request: {}", e),
            },
        };
        transport.send(&serde_json::to_value(&response).map_err(|e| e.to_string())?)?;
    }
}

/// Challenge-response login; returns the account name on success.
fn authenticate(
    transport: &mut Transport,
    shared: &Shared,
    hello: &Value,
    device_id: &str,
    deadline: Instant,
) -> Result<(String, [u8; 32]), String> {
    // Only well-formed names are looked up or logged: the value comes from an unauthenticated peer
    let username = hello
        .get("username")
        .and_then(|v| v.as_str())
        .filter(|name| valid_username(name))
        .unwrap_or_default()
        .to_string();
    let account = if username.is_empty() {
        None
    } else {
        shared.backend.account(&username)
    };
    let (salt, iterations) = match &account {
        Some(account) => (account.salt.clone(), account.iterations),
        None => {
            let decoy =
                unhex(&hmac_hex(&shared.decoy_key, username.as_bytes())).unwrap_or_default();
            (decoy[..16.min(decoy.len())].to_vec(), DEFAULT_ITERATIONS)
        }
    };

    let nonce = random_bytes(16).map_err(|e| e.to_string())?;
    transport.send(&json!({
        "t": "challenge",
        "salt": hex(&salt),
        "iter": iterations,
        "nonce": hex(&nonce),
    }))?;

    let reply = recv_until(transport, deadline, &shared.stop)?;
    let cnonce = reply
        .get("cnonce")
        .and_then(|v| v.as_str())
        .and_then(unhex)
        .filter(|c| c.len() == 16);
    let proof = reply.get("proof").and_then(|v| v.as_str());
    let verified = match (&account, &cnonce, proof) {
        (Some(account), Some(cnonce), Some(proof)) => {
            let transcript = transcript(&nonce, cnonce, device_id);
            constant_time_eq(
                client_proof(&account.key, &transcript).as_bytes(),
                proof.as_bytes(),
            )
        }
        _ => false,
    };
    let (Some(account), Some(cnonce), true) = (account, cnonce, verified) else {
        std::thread::sleep(DENY_DELAY);
        let _ = transport.write_line(&json!({ "t": "denied" }));
        log_warn!(
            "link: login failed (account `{}`)",
            if username.is_empty() {
                "<invalid>"
            } else {
                &username
            }
        );
        return Err("login failed".to_string());
    };

    let transcript = transcript(&nonce, &cnonce, device_id);
    transport.send(&json!({
        "t": "ready",
        "auth": "password",
        "proof": server_proof(&account.key, &transcript),
    }))?;
    transport.key = Some(session_key(&account.key, &transcript));
    Ok((username, account.key))
}

// ── Client (AP / Gateway) ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Loopback,
    Password,
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthMode::Loopback => "loopback",
            AuthMode::Password => "password",
        }
    }
}

pub struct Session {
    transport: Transport,
    pub auth: AuthMode,
}

impl Session {
    /// Connect and log in. `credentials` is only needed for a non-loopback Controller.
    pub fn connect(
        endpoint: &str,
        device_id: &str,
        credentials: Option<&Credentials>,
    ) -> Result<Self, String> {
        let addrs: Vec<SocketAddr> = endpoint
            .to_socket_addrs()
            .map_err(|e| format!("cannot resolve `{}`: {}", endpoint, e))?
            .collect();
        let mut last_error = format!("`{}` resolves to no address", endpoint);
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(stream) => return Self::handshake(stream, device_id, credentials),
                Err(e) => last_error = format!("cannot connect to {}: {}", addr, e),
            }
        }
        Err(last_error)
    }

    fn handshake(
        stream: TcpStream,
        device_id: &str,
        credentials: Option<&Credentials>,
    ) -> Result<Self, String> {
        stream
            .set_read_timeout(Some(CLIENT_TIMEOUT))
            .and_then(|_| stream.set_write_timeout(Some(CLIENT_TIMEOUT)))
            .map_err(|e| format!("configuring the socket failed: {}", e))?;
        let _ = stream.set_nodelay(true);
        let peer_is_loopback = stream
            .peer_addr()
            .map(|a| is_loopback(a.ip()))
            .map_err(|e| format!("no peer address: {}", e))?;
        let mut transport = Transport::new(stream, C2S, S2C);

        transport.send(&json!({
            "t": "hello",
            "v": PROTOCOL_VERSION,
            "device_id": device_id,
            "username": credentials.map(|c| c.username.as_str()).unwrap_or(""),
        }))?;

        let reply = Self::expect(&mut transport)?;
        match reply.get("t").and_then(|v| v.as_str()) {
            Some("ready") if reply.get("auth").and_then(|v| v.as_str()) == Some("loopback") => {
                if !peer_is_loopback {
                    return Err(
                        "the Controller offered a login-free session on a non-loopback connection; refusing"
                            .to_string(),
                    );
                }
                transport.max_line = MAX_LINE;
                Ok(Self {
                    transport,
                    auth: AuthMode::Loopback,
                })
            }
            Some("challenge") => {
                let credentials = credentials.ok_or(
                    "the Controller requires an account: set the account name and password",
                )?;
                Self::login(transport, &reply, device_id, credentials)
            }
            Some("error") => Err(reply
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("the Controller refused the connection")
                .to_string()),
            _ => Err("unexpected answer from the Controller".to_string()),
        }
    }

    fn expect(transport: &mut Transport) -> Result<Value, String> {
        transport.deadline = Some(Instant::now() + CLIENT_TIMEOUT);
        transport
            .recv()?
            .ok_or_else(|| "the Controller did not answer in time".to_string())
    }

    fn login(
        mut transport: Transport,
        challenge: &Value,
        device_id: &str,
        credentials: &Credentials,
    ) -> Result<Self, String> {
        let field = |name: &str| challenge.get(name).and_then(|v| v.as_str()).and_then(unhex);
        let salt = field("salt").ok_or("invalid challenge")?;
        let nonce = field("nonce").ok_or("invalid challenge")?;
        let iterations = challenge
            .get("iter")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| (1..=1_000_000).contains(v))
            .ok_or("invalid challenge")?;

        let key = derive_key(credentials.password.as_bytes(), &salt, iterations);
        let cnonce = random_bytes(16).map_err(|e| e.to_string())?;
        let transcript = transcript(&nonce, &cnonce, device_id);
        transport.send(&json!({
            "t": "auth",
            "cnonce": hex(&cnonce),
            "proof": client_proof(&key, &transcript),
        }))?;

        let reply = Self::expect(&mut transport)?;
        match reply.get("t").and_then(|v| v.as_str()) {
            Some("ready") => {
                let proof = reply.get("proof").and_then(|v| v.as_str()).unwrap_or("");
                if !constant_time_eq(server_proof(&key, &transcript).as_bytes(), proof.as_bytes()) {
                    return Err("the Controller failed to prove its identity".to_string());
                }
                transport.key = Some(session_key(&key, &transcript));
                transport.max_line = MAX_LINE;
                Ok(Self {
                    transport,
                    auth: AuthMode::Password,
                })
            }
            Some("denied") => Err("the Controller rejected the account or password".to_string()),
            Some("error") => Err(reply
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("login failed")
                .to_string()),
            _ => Err("unexpected answer from the Controller".to_string()),
        }
    }

    pub fn sync(&mut self, request: &SyncRequest) -> Result<SyncResponse, String> {
        let message =
            serde_json::to_value(Request::Sync(request.clone())).map_err(|e| e.to_string())?;
        self.transport.send(&message)?;
        let reply = Self::expect(&mut self.transport)?;
        match serde_json::from_value::<Response>(reply) {
            Ok(Response::Sync(response)) => Ok(response),
            Ok(Response::Error { message }) => Err(message),
            Err(e) => Err(format!("invalid answer: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::AccountStore;
    use std::sync::Mutex;
    use wifisync_core::admission::AdmissionState;
    use wifisync_core::link::{NodeInfo, NodeRole};

    struct TestBackend {
        accounts: AccountStore,
        serving: AtomicBool,
        seen: Mutex<Vec<Peer>>,
    }

    impl Backend for TestBackend {
        fn serving(&self) -> bool {
            self.serving.load(Ordering::Relaxed)
        }
        fn account(&self, username: &str) -> Option<AccountKey> {
            self.accounts.lookup(username)
        }
        fn sync(&self, peer: &Peer, request: SyncRequest) -> Response {
            self.seen.lock().unwrap().push(peer.clone());
            Response::Sync(SyncResponse {
                server_time: 99,
                admission: request
                    .node
                    .has_role(NodeRole::Ap)
                    .then_some(AdmissionState::Pending),
                ..SyncResponse::default()
            })
        }
    }

    fn backend(tag: &str) -> (Arc<TestBackend>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("wifisync-link-{}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        let accounts = AccountStore::new(dir.join("accounts.json"));
        accounts.add("ap1", "correct horse", 1).unwrap();
        (
            Arc::new(TestBackend {
                accounts,
                serving: AtomicBool::new(true),
                seen: Mutex::new(Vec::new()),
            }),
            dir,
        )
    }

    fn server(backend: Arc<TestBackend>, trust_loopback: bool) -> ServerHandle {
        start_server(
            "127.0.0.1:0".parse().unwrap(),
            backend,
            ServerOptions { trust_loopback },
        )
        .unwrap()
    }

    fn request(roles: Vec<NodeRole>) -> SyncRequest {
        SyncRequest {
            node: NodeInfo {
                device_id: "dev-1".into(),
                roles,
                ..NodeInfo::default()
            },
            ..SyncRequest::default()
        }
    }

    fn creds(user: &str, password: &str) -> Credentials {
        Credentials {
            username: user.into(),
            password: password.into(),
        }
    }

    fn connect(
        server: &ServerHandle,
        credentials: Option<&Credentials>,
    ) -> Result<Session, String> {
        Session::connect(&server.addr().to_string(), "dev-1", credentials)
    }

    #[test]
    fn loopback_peers_skip_authentication() {
        let (backend, dir) = backend("loopback");
        let server = server(backend.clone(), true);
        let mut session = connect(&server, None).unwrap();
        assert_eq!(session.auth, AuthMode::Loopback);
        let response = session.sync(&request(vec![NodeRole::Ap])).unwrap();
        assert_eq!(response.server_time, 99);
        assert_eq!(response.admission, Some(AdmissionState::Pending));
        let seen = backend.seen.lock().unwrap();
        assert!(seen[0].loopback);
        assert!(seen[0].username.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn password_login_and_encrypted_session() {
        let (backend, dir) = backend("login");
        let server = server(backend.clone(), false);
        let credentials = creds("ap1", "correct horse");
        let mut session = connect(&server, Some(&credentials)).unwrap();
        assert_eq!(session.auth, AuthMode::Password);
        // Several requests over the same encrypted session keep the sequence in step
        for _ in 0..3 {
            session.sync(&request(vec![NodeRole::Gateway])).unwrap();
        }
        let seen = backend.seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(!seen[0].loopback);
        assert_eq!(seen[0].username.as_deref(), Some("ap1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wrong_password_unknown_user_and_missing_credentials_fail() {
        let (backend, dir) = backend("denied");
        let server = server(backend.clone(), false);
        assert!(connect(&server, Some(&creds("ap1", "wrong password"))).is_err());
        assert!(connect(&server, Some(&creds("nobody", "correct horse"))).is_err());
        let error = connect(&server, None).err().unwrap();
        assert!(error.contains("requires an account"), "{}", error);
        assert!(backend.seen.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_device_without_the_controller_role_refuses_connections() {
        let (backend, dir) = backend("norole");
        backend.serving.store(false, Ordering::Relaxed);
        let server = server(backend, true);
        let error = connect(&server, None).err().unwrap();
        assert!(error.contains("not a Controller"), "{}", error);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_session_cannot_speak_for_another_device() {
        let (backend, dir) = backend("spoof");
        let server = server(backend, true);
        let mut session = connect(&server, None).unwrap();
        let mut spoofed = request(vec![NodeRole::Ap]);
        spoofed.node.device_id = "dev-2".into();
        let error = session.sync(&spoofed).err().unwrap();
        assert!(error.contains("device_id"), "{}", error);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_requests_are_answered_with_an_error() {
        let (backend, dir) = backend("invalid");
        let server = server(backend, true);
        let mut session = connect(&server, None).unwrap();
        let mut bad = request(vec![NodeRole::Ap]);
        bad.node.roles.clear();
        assert!(session.sync(&bad).is_err());
        // The session survives a rejected request
        assert!(session.sync(&request(vec![NodeRole::Ap])).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stopping_the_server_releases_the_port() {
        let (backend, dir) = backend("stop");
        let server = server(backend.clone(), true);
        let addr = server.addr();
        server.stop();
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            TcpListener::bind(addr).is_ok(),
            "the port must be free again"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn removing_an_account_ends_its_live_session() {
        let (backend, dir) = backend("revoke");
        let server = server(backend.clone(), false);
        let credentials = creds("ap1", "correct horse");
        let mut session = connect(&server, Some(&credentials)).unwrap();
        session.sync(&request(vec![NodeRole::Gateway])).unwrap();

        backend.accounts.remove("ap1").unwrap();
        assert!(session.sync(&request(vec![NodeRole::Gateway])).is_err());
        // The Controller did not serve the request after the removal
        assert_eq!(backend.seen.lock().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn changing_the_password_ends_live_sessions_too() {
        let (backend, dir) = backend("rekey");
        let server = server(backend.clone(), false);
        let credentials = creds("ap1", "correct horse");
        let mut session = connect(&server, Some(&credentials)).unwrap();
        backend
            .accounts
            .set_password("ap1", "battery staple")
            .unwrap();
        assert!(session.sync(&request(vec![NodeRole::Gateway])).is_err());
        assert!(connect(&server, Some(&creds("ap1", "battery staple"))).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_hostile_user_name_is_neither_looked_up_nor_logged() {
        let (backend, dir) = backend("username");
        let server = server(backend, false);
        let hostile = creds("ap1\nwifisync[1]: forged", "correct horse");
        assert!(connect(&server, Some(&hostile)).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn oversized_pre_login_frames_are_refused() {
        let (backend, dir) = backend("preauth");
        let server = server(backend, false);
        let mut stream = TcpStream::connect(server.addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        // Far more than a hello can be, without a newline
        let blob = vec![b'a'; PRE_AUTH_LINE + 1024];
        let _ = stream.write_all(&blob);
        let mut sink = [0u8; 64];
        // The Controller drops the connection (EOF) or resets it
        let closed = matches!(stream.read(&mut sink), Ok(0) | Err(_));
        assert!(closed);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_peer_that_drips_bytes_cannot_hold_the_handshake_open() {
        let (backend, dir) = backend("drip");
        let server = server(backend, false);
        let mut stream = TcpStream::connect(server.addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        // One byte every 300 ms keeps every individual read alive; the handshake window is what
        // ends the connection
        let started = Instant::now();
        let mut ended = false;
        while started.elapsed() < HANDSHAKE_TIMEOUT + Duration::from_secs(4) {
            if stream.write_all(b"x").is_err() {
                ended = true;
                break;
            }
            let mut byte = [0u8; 1];
            if matches!(stream.read(&mut byte), Ok(0)) {
                ended = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        assert!(
            ended,
            "the connection must be closed within the handshake window"
        );
        assert!(started.elapsed() < HANDSHAKE_TIMEOUT + Duration::from_secs(4));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn one_address_cannot_use_all_connection_slots() {
        let (backend, dir) = backend("perip");
        let server = server(backend, true);
        let mut open = Vec::new();
        for _ in 0..MAX_PER_IP {
            open.push(TcpStream::connect(server.addr()).unwrap());
        }
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(server.active(), MAX_PER_IP);
        // Over the cap the Controller drops the new connection without serving it
        let mut extra = TcpStream::connect(server.addr()).unwrap();
        extra
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut sink = [0u8; 8];
        assert!(matches!(extra.read(&mut sink), Ok(0) | Err(_)));
        assert_eq!(server.active(), MAX_PER_IP);
        drop(open);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn pair() -> (Transport, Transport) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        for stream in [&client, &server] {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
        }
        (
            Transport::new(client, C2S, S2C),
            Transport::new(server, S2C, C2S),
        )
    }

    #[test]
    fn encrypted_frames_reject_replay_and_tampering() {
        let (mut client, mut server) = pair();
        let key = vec![7u8; 32];
        client.key = Some(key.clone());
        server.key = Some(key);

        client.send(&json!({"a": 1})).unwrap();
        client.send(&json!({"a": 2})).unwrap();
        assert_eq!(server.recv().unwrap().unwrap(), json!({"a": 1}));
        // Pretend the first frame is delivered again: the sequence no longer matches
        server.recv_seq = 0;
        assert!(server.recv().is_err());
    }

    #[test]
    fn a_wrong_session_key_cannot_read_frames() {
        let (mut client, mut server) = pair();
        client.key = Some(vec![1u8; 32]);
        server.key = Some(vec![2u8; 32]);
        client.send(&json!({"secret": true})).unwrap();
        assert!(server.recv().is_err());
    }

    #[test]
    fn frames_never_expose_the_payload_on_the_wire() {
        let (mut client, mut server) = pair();
        client.key = Some(vec![9u8; 32]);
        client.send(&json!({"password": "hunter2hunter2"})).unwrap();
        // Read the raw line the way an eavesdropper would
        let line = server.read_line().unwrap().unwrap();
        let text = String::from_utf8(line).unwrap();
        assert!(!text.contains("hunter2"));
        assert!(text.contains("\"d\""));
    }
}
