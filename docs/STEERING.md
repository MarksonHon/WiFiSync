**English** | [简体中文](STEERING_zh-cn.md)

# Client Steering Design

> Status: **proposed — not implemented yet.**
> Scope: a self-contained client steering subsystem built into `wifisync` (no external daemon).
> Hard prerequisite: the full `wpad` package (hostapd ubus interface) — already a package dependency.
> Default: **off**; first enablement is dry-run only.

Related documents: [`BACKEND.md`](BACKEND.md) (roles, KVR provisioning, lifecycle) ·
[`FRONTEND.md`](FRONTEND.md) (ubus contract) · [`BUILDING.md`](BUILDING.md) (build and CI).

---

## 1. Positioning

Steering here means: **while the service is running**, watch the client stations, and when a client
would clearly be better off on another BSS of the same ESS, ask it to move (802.11v BSS Transition
Management) or — only when explicitly enabled — force it.

### 1.1 Relationship to the existing design

`wifisync` today provisions 802.11k/v/r parameters and distributes network information. It never
interferes with a running client. Steering adds a **runtime actuation** layer on top of that.

Nothing in the existing flow changes: the configuration plane (roles, admission, bridge, KVR,
backup/restore) keeps its current semantics. Steering is an additional, independently switchable
subsystem.

### 1.2 Non-goals

* No mesh (802.11s) — unchanged.
* No radio planning, channel selection, or spectrum management.
* No client-side agent; only standard 802.11k/v signalling.
* No writing of network configuration: steering never touches uci `network`/`wireless` beyond what
  the existing KVR/bridge flow already does.
* No adoption of `usteer`/`DAWN`: the project constraint is "no external code or external
  components", and both are standalone daemons with their own config and init scripts.

### 1.3 Paradigm deviation (recorded on purpose)

`BACKEND.md` states three cross-cutting principles: **zero intrusion**, **reversible**, **least
responsibility**. Steering deviates from two of them, so the deviation is recorded here:

| Principle | Before | After steering |
|-----------|--------|----------------|
| Zero intrusion | No runtime interference at all | **Network configuration stays untouched**, but a client may be asked to move. Off by default. |
| Reversible | Every change can be restored from the baseline | A steered client may fail to reconnect. Compensated by dry-run, allow-lists, rate limits and a kill switch — not truly reversible. |
| Least responsibility | Controller validates/distributes, AP applies | Unchanged in shape: **controller decides, AP executes**. |

Consequence: steering is **off by default**, its first mode is dry-run, and §8 applies in full.

### 1.4 Constraints taken from the project rules

* No new third-party crates or binaries. Only `ubus` (already used for diagnostics) and the full
  `wpad` hostapd interface (already a dependency).
* Any new CLI output and log line is plain English; only the LuCI language packages are localized.
* LuCI contains no steering logic: it only calls ubus methods declared in `common.js`.

---

## 2. Architecture — controller decides, AP executes

`usteer` and `DAWN` are peer-to-peer: every AP keeps its own view of the stations, exchanges it with
its peers (UDP broadcast / TCP + uMDNS), and decides locally. They have no central node, so they
must solve peer discovery and state consistency themselves.

`wifisync` already **has** a central node (the controller), an authenticated channel, admission, and
a versioned distribution mechanism (R8/R9). Steering therefore inverts the topology:

```
             ┌────────────── Controller (decision maker) ──────────────┐
             │  station view aggregation → policy evaluation → decision │
             │  (reason + evidence recorded for every evaluation)       │
             └───────▲───────────────────────────────────────┬─────────┘
                     │ StationReport (HMAC + ChaCha20-Poly1305)│ SteerCommand (versioned)
          ┌──────────┴──────────┐                  ┌──────────┴──────────┐
          │ AP (executor)       │                  │ AP (executor)       │
          │ hostapd ubus adapter│                  │ hostapd ubus adapter│
          └─────────────────────┘                  └─────────────────────┘
```

Design consequences:

* **No peer discovery, no consensus.** An AP never talks to another AP.
* **Single decision point ⇒ explainable.** Every decision lives in one log with its evidence, which
  is the main advantage over reusing an external daemon.
* **The controller is not a safety-critical single point.** If it is unreachable, an AP simply stops
  steering (§8.5). The safe default is "do not interfere", so a controller outage degrades to the
  current behaviour instead of an unsafe one.

---

## 3. Data model (`wifisync-core::steering`)

```rust
/// One observation of one station on one BSS, produced by an AP.
pub struct StationObservation {
    pub observed_at_ms: u64,   // monotonic local clock
    pub ap_id: String,         // device_id (same identity used by admission)
    pub bssid: String,
    pub ssid: String,
    pub freq: u32,
    pub signal_dbm: i32,
    pub connected: bool,       // associated with this BSS right now
    pub num_sta: u32,          // stations on this radio (load input)
    pub channel_load: Option<u8>,
}

/// Controller-side aggregation: client MAC -> all observations of the last window.
pub struct StationView { /* BTreeMap<Mac, Vec<StationObservation>> */ }

pub struct SteeringPolicy {
    pub enabled: bool,
    pub dry_run: bool,                 // default true
    pub min_snr_dbm: i32,              // only consider clients below this
    pub min_gain_db: i32,              // target must be this much better
    pub hysteresis: u8,                // consecutive identical evaluations
    pub cooldown_ms: u64,              // per-client minimum interval
    pub max_steers_per_hour: u32,      // per-client rate limit
    pub aggressiveness: Aggressiveness, // Btm (default) | BtmDisassoc | Kick (opt-in)
    pub ssid_allowlist: Vec<String>,
    pub mac_denylist: Vec<String>,
}

pub enum Decision {
    Hold { reason: Reason },
    Steer { target_bssid: String, target_ap: String, reason: Reason, evidence: Evidence },
}
```

`Decision` always carries a machine-readable `reason` plus the evidence (both sides' signal, load,
policy version). This is what makes §9.S7 testable.

---

## 4. Inter-node protocol (reuses the existing channel)

Two new message types on the existing HMAC-SHA256 + ChaCha20-Poly1305 channel — no new transport,
no new credentials:

| Message | Direction | Contents | Notes |
|---------|-----------|----------|-------|
| `StationReport` | AP → Controller | `version`, `ap_id`, monotonic timestamp, batch of `StationObservation` | Default every 5 s; an observation is included when it is connected or its signal moved by more than 3 dB |
| `SteerCommand` | Controller → AP | `policy_version`, `mac`, `target_bssid`, `aggressiveness`, `validity_ms`, `decision_id` | AP rejects anything expired, stale-versioned, or outside the allow-list |

Rules:

* Timestamps are monotonic and validated against a window on receipt (replay protection), consistent
  with the existing heartbeat HMAC handling.
* `policy_version` is monotonically increasing, mirroring `NetworkProfile.version`; APs ignore
  commands stamped with an older version.
* The AP **validates before acting**: the target BSSID must currently exist on that AP, the client
  must be associated, and the rate limit must not be exceeded.

---

## 5. AP-side hostapd adapter (`wifisync-sys`)

Everything goes through the `ubus` CLI, matching the existing `exec::run("uci" | "iwinfo" | "ip", …)`
style. The ubus binary protocol (blob encoding) is **not** reimplemented.

| Purpose | Command |
|---------|---------|
| Enable 802.11k/v capabilities at runtime | `ubus call hostapd.<iface> bss_mgmt_enable '{ "neighbor_report": true, "beacon_report": true, "link_measurement": true, "bss_transition": true }'` |
| Read connected stations | `ubus call hostapd.<iface> get_clients` |
| Read per-client detail | `ubus call hostapd.<iface> get_client_info '{ "addr": "<mac>" }'` |
| Own neighbor report | `ubus call hostapd.<iface> rrm_nr_get_own` |
| Publish the neighbor report list | `ubus call hostapd.<iface> rrm_nr_set '{ … }'` |
| Ask a client to move (default) | `ubus call hostapd.<iface> bss_transition_request '{ … }'` |
| Force a move (opt-in only) | `ubus call hostapd.<iface> wnm_disassoc_imminent '{ … }'` |
| Subscribe to hostapd notifications | `ubus subscribe hostapd.<iface>` (child process, JSON per line) |

Adapter requirements:

* **Field-name trap:** the hostapd README spells the argument `link_measurements`, the hostapd source
  uses `link_measurement` (singular). The singular form is authoritative; the plural silently does
  nothing.
* `bss_mgmt_enable` must be re-issued whenever hostapd restarts, and `ubus subscribe` must be
  re-attached (`ubus wait_for hostapd.<iface>` before subscribing).
* The child process is supervised like any other external command; a dead subscriber is restarted
  with backoff rather than taking the daemon down.

### 5.1 Configuration-side prerequisite (currently broken)

802.11v is **not** enabled by the uci option name in use today:

| uci option | Actual effect (OpenWrt `wifi-scripts`, hostapd config generation) |
|------------|---------------------------------------------------------------------|
| `ieee80211k 1` | emits `rrm_neighbor_report=1` and `rrm_beacon_report=1` |
| **`bss_transition 1`** | emits hostapd `bss_transition=1` — this is the 802.11v switch |
| `ieee80211v 1` | no handler was found in `hostapd.sh` / `ap.uc`; treated as a no-op until proven otherwise |
| `ieee80211r 1` | plus `mobility_domain`, `ft_over_ds`, `ft_psk_generate_local` — already emitted correctly |

So the current write plan (`ieee80211k` / `ieee80211v` / `ieee80211r`) does **not** turn on BTM,
while steering depends on it. Fixing this is milestone S0, for both steering and plain 802.11v
correctness.

---

## 6. Controller-side policy

Evaluation loop (per station, per policy tick, default 5 s):

1. Build the station view from reports within the freshness window; drop stale APs.
2. Ignore stations that are: on a non-allow-listed SSID, in the MAC denylist, inside the cooldown, or
   above `max_steers_per_hour`.
3. Candidate selection: current signal below `min_snr_dbm`, and a target BSS of the same SSID on
   another AP whose signal is better by at least `min_gain_db`.
4. Require `hysteresis` consecutive identical decisions before acting (anti-flap).
5. Emit `Hold` or `Steer` — always with reason and evidence, regardless of `dry_run`.

Deliberately **not** implemented: a weighted multi-factor score (the DAWN model). The inputs we can
obtain reliably — signal, band, station count, and optionally channel load — are enough for a home
ESS, and a threshold policy with explicit hysteresis is far easier to reason about and to debug.

---

## 7. Configuration (uci)

New section in `/etc/config/wifisync`, **disabled by default**:

```
config steering 'steering'
	option enabled '0'
	option dry_run '1'
	option min_snr_dbm '-70'
	option min_gain_db '8'
	option hysteresis '3'
	option cooldown_ms '60000'
	option max_steers_per_hour '4'
	option aggressiveness 'btm'
	option report_interval_ms '5000'
	option controller_timeout_ms '30000'
	list   ssid_allowlist 'Home'
	list   mac_denylist ''
```

`aggressiveness` values: `btm` (default, polite request), `btm_disassoc` (request with imminent
disassociation), `kick` (forced disassociation; explicit opt-in only).

---

## 8. Safety rails

1. **Off by default**; enabling requires an explicit confirmation in LuCI.
2. **Dry-run by default** — decisions are logged but nothing is sent to a client.
3. **Kill switch**: an immediate RPC that stops all steering without restarting the service.
4. **Per-client rate limit** (`max_steers_per_hour`) plus cooldown between attempts.
5. **AP-side autonomy on loss**: an AP stops executing whenever the controller has been unreachable
   for `controller_timeout_ms`. Safe default, no coordination needed.
6. **Allow-list / denylist** by SSID and by MAC; enforced on the controller *and* re-checked on the AP.
7. **Steering never participates in failsafe**: it must not trigger or block an apply-guard rollback
   or a network restore.
8. **Relay topology downgrade**: on a relay link (where 802.11r is already restricted), steering is
   disabled by default and this is surfaced in the UI.

---

## 9. Requirement traceability

| # | Requirement | Design highlights | Modules | Acceptance |
|---|-------------|-------------------|---------|------------|
| S1 | 802.11v must be genuinely enabled | Emit `bss_transition`, plus runtime `bss_mgmt_enable` | `core::plan`, `sys::hostapd` | the generated hostapd config contains `bss_transition=1`; BTM request reaches a test client |
| S2 | AP reports station observations to the controller | `StationReport` on the existing authenticated channel | `sys::hostapd`, `daemon` | a two-AP hwsim setup yields both APs' observations on the controller |
| S3 | Controller builds a global station view | Windowed aggregation, stale-AP expiry | `core::steering` | unit tests + live view in the UI |
| S4 | Policy produces explainable decisions | Thresholds + hysteresis + cooldown + allow/deny lists | `core::steering` | identical evaluation series produce identical decisions (deterministic tests) |
| S5 | AP executes a steer command | BTM by default; `wnm_disassoc_imminent` opt-in | `sys::hostapd` | hwsim handover succeeds; a client that ignores BTM is not disturbed further |
| S6 | Dry-run and kill switch | Log-only mode; immediate stop RPC | `daemon`, `rpc` | with `dry_run=1` the network diff stays 0 and no client is affected |
| S7 | Every decision is explainable | `reason` + evidence + policy version recorded | `core::steering`, `rpc` | each log line answers "why this client, why this target" |
| S8 | Fail-safe on controller loss | AP self-stops after the timeout | `daemon` | unplugging the controller stops steering on the APs |
| S9 | No new external dependency | ubus CLI + full wpad only | `Cargo.toml`, `Makefile` | dependency list and size gate unchanged |
| S10 | LuCI page | ubus-only view, no logic; language packages | `luci-app-wifisync` | ACL/method walkthrough matches `common.js`; `zh_Hans` package covers new strings |

---

## 10. Milestones

| # | Content | Acceptance |
|---|---------|------------|
| **S0** | Foundation spikes: is 802.11v really on; does `ubus subscribe hostapd.*` deliver the events we need; what does `get_clients` return; does a BTM request actually move a station under hwsim | a spike report plus at least one real BTM handover evidence |
| **S1** | `wifisync-sys` hostapd adapter (read, subscribe, write) | unit tests plus an integration test against a **real** hostapd, not a stub |
| **S2** | `StationReport` upload and controller-side aggregation (no decisions) | two hwsim APs aggregated into one view |
| **S3** | Policy + decisions + dry-run (log only) | logs explain every decision; zero actual intervention |
| **S4** | Execution path (BTM, lowest aggressiveness) + safety rails | hwsim handover; kill switch and controller-loss stop verified |
| **S5** | LuCI page, documentation, and the recorded paradigm deviation | page can dry-run, switch off, and show reasons |

---

## 11. Tests

* **Unit**: policy functions — hysteresis, cooldown, anti-flap, allow/deny lists, rate limiting.
* **Integration (real chain)**: QEMU + hwsim, three nodes (gateway/controller/AP) with two APs.
  The hostapd adapter is exercised against a **real** hostapd; a stub that returns canned JSON would
  hide exactly the failures this feature is about.
* **Regression**: with `steering.enabled=0` the observable behaviour is byte-identical to today;
  with `dry_run=1` no client is affected and the network diff stays 0.
* **Negative**: controller offline ⇒ AP stops; a client that does not support BTM ⇒ untouched.

---

## 12. Risks

| Risk | Mitigation |
|------|------------|
| A forced move can leave a client unable to reconnect | BTM soft request by default; `kick` requires explicit opt-in, an allow-list, and a rate limit |
| Thresholds cannot be inherited from an existing project — they must be calibrated on the real topology | S3 dry-run collects data before anything is executed |
| Binary size may exceed the 3 MiB CI gate | measure right after S1; if needed, ship steering as a separate binary in the same package |
| Relay links restrict 802.11r and will restrict steering too | automatic downgrade, surfaced in the UI (§8.8) |
| 802.11v silently disabled (see §5.1) | S0 fixes the configuration chain first |
| `ubus` CLI overhead per call | batch reads are unnecessary; a single `get_clients` per radio per cycle, and steer calls are rare by construction |

---

## 13. Open items before implementation

1. Confirm on the target OpenWrt build whether `ieee80211v` is a no-op, and whether `bss_transition`
   is required in `plan.rs` (§5.1).
2. Confirm which notification types `ubus subscribe hostapd.<iface>` actually delivers, and their
   output shape.
3. Confirm the exact output schema of `get_clients` / `get_client_info` on the target wpad build.
4. Confirm the `bss_transition_request` argument set accepted by that hostapd build.
