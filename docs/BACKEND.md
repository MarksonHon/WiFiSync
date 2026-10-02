**English** | [简体中文](BACKEND_zh-cn.md)

# Backend Design

> Language: Rust (single binary, musl) · Platform: OpenWrt 25.12.x (x86 / ARM64) · Service name: `wifisync`
> Positioning: **no mesh** — a single home network is built from "Controller admission + network
> information distribution + 802.11k/v/r roaming + standard Wi-Fi relay".
> Core principle: **zero intrusion by default** — only a "pure AP" device is modified by the program;
> Gateway/Controller only read, probe, and distribute.
> Lifecycle discipline: **back up the initial network before start, restore the previous network before stop**.

The LuCI interface is documented separately in [`FRONTEND.md`](FRONTEND.md); build and CI details
are in [`BUILDING.md`](BUILDING.md).

---

## 1. Requirements traceability (Requirements → Design → Acceptance)

| # | Requirement | Design highlights | Modules | Acceptance |
|---|-------------|-------------------|---------|------------|
| R1 | Include a LuCI interface | The daemon only exposes a local UNIX-socket JSON API → the `rpcd` exec plugin bridges it to a **ubus object** → LuCI JS calls it with `rpc.declare()` (no new protocol) | `luci/luci-app-wifisync/`, `files/usr/libexec/rpcd/wifisync` | ubus `list/call` walkthrough + browser regression (see [`FRONTEND.md`](FRONTEND.md)) |
| R2 | Do not use libraries that fail to build with musl | Dependency allow-list + pure-Rust cryptography + musl linking, see §4 | whole project, `.cargo/config.toml` | all musl targets compile + single binary ≤ 3 MiB |
| R3 | Roles Controller/AP/Gateway; AP disabled without wireless | A role is a **capability set**; without wireless the default roles drop AP and the UI greys it out | `core::role`, `sys::capability` | a wireless-less device defaults to `{controller,gateway}`; the AP checkbox is disabled |
| R4 | Support KVR, not mesh; relay via standard Wi-Fi relay | Emit hostapd/wpa_supplicant KVR parameters; relay = `wpa_supplicant(STA)` + `hostapd(AP)` (+`relayd`), **no 802.11s** | `core::profile`, `core::plan` | hwsim two-AP roaming logs / handover latency; no mesh branch in the code |
| R5 | ⚠️ Narrowed: **only a "pure AP"** merges all ports into `br-lan` | Bridge condition = `ap && !gateway && !controller`; any other combination **creates no bridge and touches no port** | `core::bridge`, `core::plan` | in the pure-AP case all ports join `br-lan`; for `ap+controller` / `ap+gateway` / `controller+gateway` the dry-run diff is empty |
| R6 | Optional: restore the default network when Controller/Gateway is unreachable | Heartbeat + dead-man timer + rollback engine (three layers), off by default; **AP devices only** | `core::failsafe`, `daemon` | after the link timeout the device restores the default network and becomes reachable again |
| R7 | Gateway **never changes any network setting for the user**, it only asks for the LAN interface | Gateway = read-only identification role: the user selects the "corresponding LAN interface" for topology/alerting; the write plan is always empty | `core::role`, `wifisync-sys` | after selecting a LAN port the local uci/network diff stays **0 bytes** |
| R8 | Controller by default only does "new AP validation" + "network information distribution", without touching the existing network | Controller writes no local network configuration; connectivity to the Gateway is **confirmed by the user** (the program only probes, and never changes routes/NAT/firewall) | `core::admission`, `core::plan`, `wifisync::probe` | the admission list can be approved/rejected; the connectivity check is a read-only probe plus an explicit confirmation badge |
| R9 | AP by default syncs the network information distributed by the Controller (including Wi-Fi); the Wi-Fi source is one of three choices | Sources: `controller_self` / `gateway` / `custom`; a source device reporting "no Wi-Fi" disables that source; choosing `custom` while AP and Controller are on the **same device** also modifies that device's Wi-Fi | `core::wifi_source`, `core::profile` | the three sources switch and disable correctly; the same-device `custom` case generates the right wireless configuration (with double confirmation) |
| R10 | Not hard-coded: reserve multi-bridge / cross-device VLAN bridge sync | Plan structures are always **lists** (`Vec<BridgePlan>`, `vlans[]`, `bridges[]`); by default exactly one `br-lan` is produced | `core::bridge`, `core::profile` | unit tests cover multi-bridge + VLAN tagged/untagged relations; no hard-coded single-bridge branch |
| R11 | **The initial network parameters must be backed up before the service starts and restored before it stops** | Lifecycle hooks: `pre_start` → create the **immutable initial baseline** `initial/` on first run (+ a rolling snapshot per start); `pre_stop` → **synchronously restore** from the baseline before exiting; includes dirty-flag detection, integrity verification, and `managed_only`/`full` restore scopes | `core::backup`, `sys::snapshot`, `sys::restore`, `etc/init.d/wifisync` | after one start/stop cycle the network configuration diff against the initial baseline is 0; after `kill -9` the next start detects the dirty state and warns/restores |

**Cross-cutting principles**

| Principle | Meaning |
|-----------|---------|
| Zero intrusion | The Gateway / Controller roles themselves **produce no network configuration writes**; only the `ap` role writes (and only after admission + double confirmation + apply-guard) |
| Explicit confirmation | Risky actions such as connectivity and same-device Wi-Fi changes are always confirmed by an explicit user click; the program only probes and previews |
| Least responsibility | Controller = validate + distribute; Gateway = identify + probe endpoints; AP = apply + report |
| Reversible | **Back up before changing, restore before exiting**: every network write is preceded by a baseline snapshot, and the service must restore the initial network before it stops (see §8) |

**UI text and message keys**

* The backend never sends finished sentences to the web UI. Hint fields (`plan.notes`,
  `bridge_blocked_reason`, `failsafe.state_label`, Wi-Fi source reasons, role adjustments,
  backup/restore summaries, apply/gateway messages, probe details, …) are
  `wifisync_core::Message` values — a stable `key` plus optional `params` — which the front end
  translates (see [`FRONTEND.md`](FRONTEND.md)).
* `WritePlan::dry_run_text()` is language neutral: it only renders the uci commands, while the
  notes travel separately as messages.
* Operational identifiers stay stable keys the UI maps itself (role names, `dirty_reason`,
  admission states).
* Everything the CLI and the service print is **plain English** — usage text, results, error
  messages (`CoreError` / `SysError`) and log lines — and is never translated; only the web UI is
  localized (see the `CLI` section in `AGENTS.md`).

---

## 2. Role model

| Role | Default | Responsibility (narrowed) | Changes to the existing local network | Bridge |
|------|---------|---------------------------|--------------------------------------|--------|
| `gateway` | on when wireless is present | **identification only**: the user selects the "corresponding LAN interface" in the UI for topology display and alerting; provides a connectivity probe endpoint | **none** (no uci writes, no netifd calls, no NAT/firewall changes) | no bridge |
| `controller` | on when wireless is present | ① **new AP validation (admission)** ② **network information distribution (including Wi-Fi)** ③ heartbeat aggregation and topology display | **none** (does not change the local network; whether it can reach the Gateway is **confirmed by the user**) | no bridge |
| `ap` | on when wireless is present; **forced off without wireless** | sync and apply the information distributed by the Controller (Wi-Fi / mobility domain), report status | applies only the distributed wireless/bridge information, and **only after admission + double confirmation** | **pure AP only** |

**Bridge policy (the only condition)**

```
enable_bridge = roles.ap && !roles.gateway && !roles.controller
```

Implementation: `wifisync-core::plan::build_write_plan` returns an empty plan for every non-AP role
combination, and the reversible side lives in `wifisync-sys::snapshot` / `wifisync-sys::restore`
driven by the daemon lifecycle hooks.

**Role decision matrix**

| Condition | Default roles | Local configuration writes | Bridge policy |
|-----------|---------------|----------------------------|---------------|
| Wireless + main router | `controller + gateway + ap` | Wi-Fi only (from the Controller distribution / selected source) | no bridge |
| Wireless + pure AP | `ap` | Wi-Fi + bridge | all ports → `br-lan` |
| Wireless + AP and Controller on the same device | `controller + ap` | Wi-Fi only (with a `custom` source, the local Wi-Fi too) | no bridge |
| Wireless + AP and Gateway on the same device | `gateway + ap` | Wi-Fi only | no bridge |
| No wireless + main router | `controller + gateway` (AP disabled/greyed out) | **none** | no bridge (only the user-selected LAN interface is recorded) |
| No wireless + pure controller | `controller` | **none** | no bridge |

---

## 3. Dependency selection (musl constraints)

| Purpose | Choice | Avoided / rationale |
|---------|--------|---------------------|
| Runtime | **std threads + blocking I/O** (no `tokio`) | The service only needs a UNIX socket + timers + threads; std is enough and it avoids a huge runtime |
| Serialization | `serde` + `serde_json` | pure Rust |
| Network configuration | **uci + ubus/netifd** (no `rtnetlink`) | Bridge setup writes uci `config device type bridge` then `ubus call network reload` — the idiomatic OpenWrt approach, which also avoids handling DSA/swconfig differences ourselves |
| State reading | `sysfs` + `/etc/board.json` + `iwinfo` | read-only; does not rely on `ip -j` (busybox `ip` does not support JSON) |
| Local API | "one JSON per line" protocol over a UNIX socket (no `httparse`/`hyper`/`axum`) | one less protocol parsing surface and fewer dependencies; the socket is isolated with mode 0600 |
| Inter-node authentication | `hmac` + `sha2` + `chacha20poly1305` | pure Rust, available on all architectures (including MIPS) |
| Signals | `libc` (only `sigaction`) | SIGTERM/SIGINT/SIGHUP; everything else goes through std |
| Errors | `thiserror` | compile-time macro, no runtime overhead |
| Credential/state storage | JSON files with atomic writes (`/etc/wifisync/`, 0600) | avoids SQLite size and musl cross-compilation pain |
| CLI parsing | hand-written (`match` in `main.rs`) | the command set is fixed, so no dependency is needed |
| ❌ Forbidden | `openssl-sys`, `ring`, `aws-lc-rs`, `git2`, `libz-sys` (non-bundled), any crate requiring `cmake` | unreliable under musl/cross-compilation and for size; **`ring` does not support MIPS/MIPSEL** |
| ⚠️ Use with care | `regex`, `libsqlite3-sys`, `tokio` with all features | significantly increase the binary size |

**Security model (why there is no TLS)**

* Local API: UNIX socket `/var/run/wifisync/wifisync.sock`, mode `0600`, root only;
* Between nodes: HMAC-SHA256 authentication + ChaCha20-Poly1305 payload encryption (pure Rust);
* **TLS is deliberately not introduced**: `openssl-sys` is painful under musl/cross-compilation,
  `ring` does not support MIPS, and `aws-lc-rs` requires cmake. HTTPS for the web UI is left to
  OpenWrt's own uhttpd.

---

## 4. Build and size constraints

| Item | Measure |
|------|---------|
| Target triples | `x86_64` / `i686` / `i586` / `aarch64` `-unknown-linux-musl` (covering all x86 and ARM64 package architectures of OpenWrt 25.12) |
| Linking | the **official OpenWrt SDK**'s `*-musl-gcc` with `-C target-feature=-crt-static` (link dynamically against the device's musl, matching the official `rust-values.mk`) |
| Size | `opt-level="z"`, `lto=true`, `codegen-units=1`, `panic="abort"`, `strip`; CI gate **3 MiB** |
| Reproducibility | `Cargo.lock` is committed and every build uses `--locked` |

The architecture mapping table and the CI workflows are documented in
[`BUILDING.md`](BUILDING.md); both CI and developers share the single implementation
`scripts/openwrt-arch.sh`.

---

## 5. KVR and relay design

| Capability | hostapd (AP side) | wpa_supplicant (STA side) | Notes |
|------------|-------------------|---------------------------|-------|
| 802.11k | `ieee80211k=1`, `rrm_neighbor_report=1` | `rrm_neighbor_report=1` | neighbor reports |
| 802.11v | `ieee80211v=1`, `bss_transition=1` | `bss_transition=1` | client steering |
| 802.11r | `ieee80211r=1`, `mobility_domain=`, `ft_over_ds=1`, `ft_psk_generate_local=1`, `r0kh`/`r1kh`/`nasid` | `ieee80211r=1`, `mobility_domain=`, `ft_psk_generate_local=1` | fast roaming, FT over DS |

* The **full `wpad`** package is required, not `wpad-basic`; an integrity check runs before start
  and prints install guidance when it is missing.
* None of the KVR parameters are **hard-coded locally**: they are distributed by the Controller
  with the `NetworkProfile.wifi[].kvr` structure (see §6), and `mobility_domain` is globally
  unique so that fast roaming works across APs.
* An AP must already be admitted before it applies Wi-Fi configuration (see §7).
* **Relay (no mesh)**: `wpa_supplicant(STA)` upstream + `hostapd(AP)` downstream; layer-2
  transparency via `relayd`, or routed mode; 4-address/WDS optional. When 802.11r is restricted
  on the relay link, fall back to `ft_over_ds`.

---

## 6. Wi-Fi information sources and network information sync (R9)

**The Controller's Wi-Fi information source is one of three choices**

| Source | How it is obtained | UI disable condition | Effect on devices |
|--------|--------------------|----------------------|-------------------|
| `controller_self` | read the Controller's own uci `wireless` + the actual iwinfo state | disabled when the Controller reports "no Wi-Fi" | does not change the Controller's own Wi-Fi (only used as a template pushed to APs) |
| `gateway` | read-only fetch of the Gateway's Wi-Fi profile (address + credentials entered by the user; **never written back to the Gateway**) | disabled when the Gateway reports "no Wi-Fi" / nothing is selected / the fetch fails | does not change the Gateway |
| `custom` | the user defines SSID, encryption, PSK, band/channel, width, KVR and `mobility_domain` manually in LuCI | none (always available) | ⚠️ if the AP and Controller are the **same device**, that device's Wi-Fi is modified as well (double confirmation + dry-run preview + apply-guard required) |

* Disabling is **validated twice**: greyed out in LuCI *and* rejected by the daemon (to prevent bypassing).
* The local Wi-Fi only enters the write plan when "Controller and AP are the same device" and the
  source is `custom`; in all other cases the Controller-side diff is 0.
* Source priority/fallback is configurable, e.g. whether a failed `gateway` fetch falls back to
  `controller_self` (by default there is no automatic fallback, only an alert).

**Distributed content: `NetworkProfile` (list-based and extensible)**

```jsonc
{
  "version": 42,                       // monotonically increasing; on conflict the higher version wins
  "bridges": [                         // a list → multiple bridges in the future
    { "name": "br-lan", "ports": ["lan1","lan2"], "vlan_filtering": false, "vlans": [] }
  ],
  "vlans": [                           // reserved: cross-device VLAN bridge sync
    { "id": 10, "name": "iot", "tagged_ports": [], "untagged_ports": ["lan3"] }
  ],
  "wifi": [                            // one profile per radio
    { "radio": "radio0", "ssid": "Home", "auth": "sae-mixed", "psk_ref": "…",
      "band": "5g", "channel": 44, "kvr": { "k": true, "v": true, "r": true,
      "mobility_domain": "abcd", "ft_over_ds": true } }
  ]
}
```

**AP sync behavior**

| Item | Default | Notes |
|------|---------|-------|
| Sync switch | `sync_mode = auto` (follow the Controller) | can be switched to `local_override` (a local temporary override with a prominent UI warning; a Controller version update does not overwrite the local value) |
| Synced content | network information (bridge/VLAN definitions) + Wi-Fi information (including KVR/mobility domain) | Wi-Fi is mandatory |
| Effective time | when the version number changes | dry-run preview + apply-guard before applying |
| Link loss | keep the last successful configuration | whether to restore the default is decided by failsafe (see §7) |

---

## 7. Controller admission and connectivity confirmation (R8)

**New AP admission flow (one of the Controller's only "active" duties)**

1. After installation the AP starts and reports `device_id` (a persisted UUID) + MAC + wireless
   capabilities + a signature made with its local credentials.
2. The Controller records it as `pending`; the LuCI "admission list" shows it (source IP, MAC,
   time, wireless capabilities).
3. The administrator **explicitly** clicks "approve" → the Controller distributes the
   `NetworkProfile`; clicking "reject" adds it to a blacklist and the AP never writes local
   configuration.
4. Credentials: a pre-shared key + heartbeat HMAC-SHA256 (with `chacha20poly1305` encrypting the
   payload); **TLS is not used** (see the dependency constraints in §3).

**Controller ↔ Gateway connectivity (confirmed by the user separately)**

| Item | Design |
|------|--------|
| Probe method | read-only probes: ICMP echo / TCP port reachability / HTTP probe bytes; the target is configurable (Gateway address or any detection point) |
| Does it change the network | **no** — it does not write the routing table, change the firewall, or touch NAT |
| User confirmation | the UI shows an "unconfirmed / confirmed" badge and requires an explicit click; the state is stored in wifisync's own uci |
| Failure handling | alert only, plus troubleshooting suggestions (no automatic repair) |

> The Controller does not participate in data-plane forwarding; the Gateway's LAN interface
> selection is only used for topology identification and probe-target inference.

---

## 8. Backup and restore lifecycle (R11)

**Core rule: back up before changing; restore before exiting.**

| Timing | Action | Notes |
|--------|--------|-------|
| `pre_start` (before any probe/write) | ① if the initial baseline does not exist → create `initial/` (**immutable**) ② if it exists → verify integrity; ③ append a rolling snapshot `pre-start-<ts>/` on every start (keeping the most recent N) | once created the baseline is never overwritten, unless the user explicitly "re-baselines" |
| `running` | take a `pre-change-<ts>/` snapshot before any write (Wi-Fi/bridge/distributed configuration) | works with apply-guard to serve L1 rollback |
| `pre_stop` (before the service stops, **synchronous, blocking, time-limited**) | restore the original network from the baseline → `uci commit` → `network restart` / `wifi reload` → verify the result → clear the runtime dirty flag → only then exit | triggered by procd `stop_service`, `SIGTERM`, the `opkg` prerm hook, and shutdown |
| Next start after an abnormal exit | detect the dirty flag ⇒ prompt (optionally automatically) to restore the baseline first | fallback for `kill -9` / power loss |

**Backup contents (all text, easy to diff and compress)**

| Item | Collection method |
|------|-------------------|
| Full uci configuration | `uci export network/wireless/dhcp/firewall/system` (per file, keeping a copy of the original `/etc/config/*`) |
| Actual wireless state | `iwinfo` / `iw dev` / `iw dev <if> info` output |
| Link state | `ip -j link`, `ip -j addr`, `ip -j route` (bridge/brport membership) |
| Metadata | `manifest.json`: time, device_id, current role set, wifisync version, **per-file sha256**, `managed_keys` (the uci key paths changed by wifisync in this run) |

```
/etc/wifisync/backup/
├─ initial/                  # immutable initial baseline (authoritative restore source)
│  ├─ manifest.json
│  ├─ config/{network,wireless,dhcp,firewall,system}
│  └─ state.txt
├─ pre-start-<ts>/           # rolling snapshot before every start (5 kept)
└─ pre-change-<ts>/          # snapshot before every write (5 kept)
```

**Restore scope and safety policy**

| Item | Default | Notes |
|------|---------|-------|
| `restore_mode` | `managed_only` | restore only the keys in `managed_keys` that wifisync changed; unrelated user configuration is preserved. `full` overwrites from the baseline as a whole (needs UI double confirmation) |
| Idempotence | required | repeated restores give the same result; sha256 is verified first, and a missing/corrupt file rejects the restore with an alert |
| Failure handling | retry once → fall back to `/rom` | a failed restore must never delete the baseline |
| Escape hatch | `wifisync daemon --no-restore-on-stop` (or `/etc/wifisync/no-restore-on-stop`) | for when the user explicitly wants to keep the current configuration (needs UI double confirmation) |
| Space control | text only + size limit | when over the limit, rolling snapshots are pruned by policy (`initial/` is never pruned) |

---

## 9. Bridge planner (narrowed + extensible, R5 / R10)

**Enable condition (default policy, overridable by future policies)**

```
enable_bridge = roles.ap && !roles.gateway && !roles.controller    // "pure AP" only
```

1. Not a pure AP ⇒ the planner returns an **empty plan** (no-op) and produces no uci/network write.
2. Pure AP ⇒ enumerate ports (prefer DSA: `/sys/class/net/*/dsa`, `board.json`; fall back to
   `swconfig`) → merge every port (including the original WAN port) into `br-lan`.
3. Output is always `Vec<BridgePlan>` with per-bridge `vlan_filtering` / `vlans[]`; by default
   exactly one `br-lan` is generated and `bridge.name` is configurable, so **no single-bridge
   assumption appears in the code**.
4. Gateway case: only record the user-selected LAN interface (`uplink.lan_ifaces`); no bridge is
   created and no WAN member is removed.
5. Landing: write uci `config device type bridge` + `netifd reload`, then verify the actual link
   with rtnetlink.
6. Snapshot uci before the change → arm apply-guard (see §10).
7. **VLAN reservation**: `VlanDef{id, name, tagged_ports, untagged_ports}` already exists in the
   profile; this version defaults to `vlans: []`. Cross-device bridge sync only needs extended
   transport and merge logic, with no model change.

---

## 10. Failover (R6, optional, off by default, AP only)

| Layer | Trigger | Action |
|-------|---------|--------|
| L1 apply-guard | connectivity is not confirmed within `confirm_timeout` after applying the distributed configuration | automatically roll back the uci snapshot + `network restart` |
| L2 link-watchdog | the **AP** loses its heartbeat with the Controller/Gateway for longer than `failsafe_timeout` | restore the network: preferably from the **initial baseline** of §8; if the baseline is unusable, fall back to `/rom/etc/config/network` + restart the network; optionally `reboot -f` |
| L3 last-resort self-healing | the daemon crashes | procd `respawn` + startup self-check; repeated failures within 24 h enter a safe mode |

* Gateway / Controller **do not trigger** a network rollback (zero-intrusion principle); they only alert.
* Configuration: `failsafe.enabled` (default `0`), `failsafe_timeout` (default `300s`),
  `failsafe_action` (`revert`|`reboot`), `failsafe_keep_ssid`, heartbeat endpoint and PSK.

---

## 11. Project structure (backend)

```
WifiSync/
├─ Cargo.toml                # workspace
├─ crates/
│  ├─ wifisync-core/         # pure logic + unit tests
│  ├─ wifisync-sys/          # uci/netifd/iwinfo/hostapd adapters (mostly read; writes only for AP)
│  │   ├─ snapshot          # uci export + iw/ip dumps + sha256 manifest + atomic write
│  │   └─ restore           # baseline restore executor (managed_only / full)
│  └─ wifisync/              # single binary: daemon + ctl + rpcd(ubus) modes
├─ openwrt/package/wifisync/ # package: Makefile + init.d + default uci + rpcd bridge + ACL
└─ scripts/                  # build-musl.sh, openwrt-arch.sh, sdk-env.sh
```

CLI entry points of the single binary `wifisync`:

```sh
wifisync daemon [--no-restore-on-stop] [--json-log]
wifisync status | plan | apply | confirm | revert
wifisync roles [<role>...] | probe ...
wifisync admission list|approve|reject|revoke
wifisync backup list|verify|create|prune
wifisync restore [--full] [--snapshot <path>]
wifisync ubus ...        # rpcd exec plugin mode
```

---

## 12. Milestones

| Phase | Goal | Main tasks | Deliverables | Acceptance | Estimate |
|-------|------|------------|--------------|------------|----------|
| **M0** Feasibility | De-risk three technical areas | ① musl static cross-compilation PoC and size measurement ② bridge/port netlink PoC ③ rpcd shim + empty LuCI page ④ hostapd KVR parameters working under `mac80211_hwsim` | `docs/spike-*.md`, runnable PoCs | all 4 PoCs run on QEMU OpenWrt x86/64 | 3–4 days |
| **M1** Core logic library | Pure logic, unit-testable, no syscalls | configuration model (UCI/JSON both ways), role decision, capability abstraction, **bridge policy (pure AP only) + multi-bridge/VLAN list model**, KVR planner, relay planner, **Wi-Fi source parser**, `NetworkProfile` versioning and merge, **admission state machine**, failsafe state machine | `crates/wifisync-core` + tests | 100% of critical branches covered; `cargo test` green on the host | 6–8 days |
| **M2** System adapter layer | Mostly read-only probing + writes only on the AP side | read-only: uci/netifd/netlink/iwinfo capabilities and topology, **Gateway "LAN interface" selection (record only, never modify)**, connectivity probes; **snapshots and restore: full `uci export` + `iw`/`ip -j` dumps + sha256 manifest + atomic write + restore executor**; writes (AP only): staging/commit, netifd reload, rtnetlink bridge/member setup, hostapd/wpa_supplicant rendering, `wpad` check; VLAN availability probing | `crates/wifisync-sys` | non-AP dry-run diff is always 0; the AP completes "admit → distribute → apply → connect"; **snapshot/restore works standalone (manual tampering is detected)** | 7–9 days |
| **M3** Daemon and safety rails | Long-running service + admission + distribution + rollback | service loop, UNIX-socket JSON-RPC, device heartbeat (PSK-HMAC), **admission service (pending/approve/reject)**, **profile distribution and version negotiation**, apply-guard dead-man timer, link-watchdog (AP only), `revert` engine, procd self-healing, **lifecycle hooks (pre_start backup / pre_stop restore / synchronous SIGTERM shutdown / dirty flag)** | `crates/wifisync`, `etc/init.d/wifisync` | a rejected/not-yet-admitted AP does not change its local configuration; an AP whose link is lost restores the default network after N seconds and can rejoin; **after one start/stop cycle the network is back to its initial state** | 8–10 days |
| **M4** LuCI application | Complete visual configuration | overview, role wizard, **Gateway (LAN port selection, zero-intrusion note)**, **Controller (admission list + Wi-Fi source choice)**, **connectivity confirmation**, bridge (visible to pure AP only), KVR, relay, failsafe, **backup and restore**, diagnostics/logs; ACL and menu registration | `luci/luci-app-wifisync` | the whole flow can be completed without a CLI; the initial baseline can be viewed/verified/restored manually | 8–10 days |
| **M5** Packaging and release | Installable ipk/apk + multiple architectures | OpenWrt package Makefile, init.d, default uci configuration, rpcd ACL, multi-target CI builds, `DEPENDS`=wpad/kmod-* | `openwrt/package/wifisync/`, CI artifacts | the official SDK builds a package that works right after opkg/apk install | 4–5 days |
| **M6** Integration and acceptance | Real-topology validation | QEMU+hwsim three-node (GW/Controller/pure AP), relay scenario, roaming latency measurement, **non-intrusiveness regression (non-AP devices diff = 0)**, **start/stop loop regression (configuration is zeroed after start→stop)**, power-loss/link-loss regression, spot checks on real hardware | test report | R1–R11 all accepted | 7–9 days |

---

## 13. Testing strategy

* **Unit tests** (host, `cargo test`): role matrix, **bridge policy (enumerate all 2³ role
  combinations and assert that only a pure AP builds a bridge)**, Wi-Fi source parsing and
  disabling, `NetworkProfile` version merge, multi-bridge/VLAN lists, KVR rendering, admission
  state machine, failsafe state machine, **`managed_keys` computation, restore plan and retention
  policy**.
* **Non-intrusiveness tests**: for the `controller` / `gateway` / `controller+gateway`
  configurations, assert that the `plan` output and the uci diff are **always empty**.
* **Lifecycle tests**: after one `start → stop` cycle the diff against the initial baseline is 0;
  after `start → change configuration → stop` the baseline is restored; `start → kill -9 → start`
  detects the dirty state and recovers; tampering with the backup files makes a restore fail.
* **dry-run**: `wifisync plan` prints the uci diff that would be written; CI compares snapshots.
* **QEMU lab**: OpenWrt x86/64 + `kmod-mac80211-hwsim` virtual wireless → topology
  (Gateway / Controller / pure AP) + relay scenario, scripted in `scripts/`.
* **Regression**: rejected admission changes nothing, link-loss recovery, power loss, concurrent
  apply, corrupt uci recovery, **20 start/stop cycles with no accumulated drift**.
* **Real-hardware spot checks**: at least one MIPS wireless router and one ARM wireless router
  (including the AP-disabled behavior on a wireless-less device).

---

## 14. Risks and mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| MIPS Tier3 targets need nightly `build-std` | complex builds | pin nightly + caching in CI; validate first in M0 |
| Small-flash devices cannot fit the binary | cannot install | size gate ≤ 3 MiB; `-Z build-std` + `panic_immediate_abort`; trim features by role if necessary |
| An AP changing the network cuts the device off | user loses connectivity | writes only for a pure AP + admission + double confirmation + dry-run + L1 apply-guard |
| Users think Gateway/Controller configure the network automatically | wrong expectations | permanent zero-intrusion text in the UI + documentation + `wifisync plan` always being empty as proof |
| Fetching Wi-Fi from the Gateway needs credentials | security/intrusion concerns | read-only fetch, credentials stored locally, explicit user authorization, **never written back** |
| `custom` source + same device ⇒ the local Wi-Fi changes | risk of losing connectivity | double confirmation + dry-run preview + apply-guard rollback + prominent warning |
| DSA/swconfig differences | bridge creation fails | dual-path detection + dry-run output comparison + rollback on failure |
| 802.11r compatibility with relay/clients | roaming anomalies | automatic fallback to `ft_over_ds`; the UI shows capability probe results |
| `wpad-basic` lacks KVR | feature unavailable | detect at startup and refuse to enable KVR, pointing to `opkg install wpad` (or the apk equivalent) |
| Future multi-bridge/VLAN needs cause model rework | refactoring cost | use list structures (`bridges[]`/`vlans[]`) from the start, with a single bridge/VLAN by default |
| **Restoring on stop overwrites later user configuration** | user configuration loss | default `restore_mode=managed_only` restores only keys changed by wifisync; `full` needs double confirmation; `--no-restore-on-stop` escape hatch |
| **A failed restore on stop makes the device unreachable** | device unreachable | sha256 verification before restoring; retry once → fall back to `/rom`; the baseline is never deleted; restore logs are kept |
| **Configuration is changed before the baseline exists** | cannot restore | `pre_start` runs before any probe/write; **fail-closed**: without a baseline any write is rejected |
| **`kill -9`/power loss leaves the network half-configured** | network in a half-configured state | dirty flag + startup self-check (prompt or automatic restore first); `pre-change` snapshots as a fallback |
| Snapshots consume flash | out of space | text only; `initial/` is never pruned, rolling snapshots keep 5 each with a size cap; when over the limit the oldest snapshots are pruned first |

---

## 15. Implementation status and deviations

### 15.1 Completed

| Milestone | Status | Output |
|-----------|--------|--------|
| M0 Feasibility | ✅ done (toolchain / arch mapping / build chain) | `scripts/openwrt-arch.sh`, `scripts/sdk-env.sh`, `scripts/build-musl.sh`, three CI workflows |
| M1 Core logic library | ✅ done | `wifisync-core`: `role` / `capability` / `bridge` / `wifi_source` / `profile` / `admission` / `backup` / `failsafe` / `plan` / `uci_file` / `config` (62 unit tests) |
| M2 System adapter layer | ✅ done (read-only probing + snapshot/restore + AP write path) | `wifisync-sys`: `sysfs` / `iwinfo` / `uci` / `netifd` / `snapshot` / `restore` / `state` / `exec` / `paths` (23 unit tests) |
| M3 Daemon and safety rails | ✅ mainly done | the single `wifisync` binary: `daemon` (lifecycle + admission + distribution + watchdog), `rpc`, `ctl`, `ubus` (rpcd plugin), `secrets`, `probe`, `signals`, `log` (14 unit tests) |
| M4 LuCI application | ✅ first version done | `luci-app-wifisync`, 8 pages + menu/ACL (JS passes `node --check`) |
| M5 Packaging and release | ✅ first version done | `openwrt/package/wifisync` (Makefile + init.d + default uci + rpcd bridge + ACL), three workflows |
| M6 Integration and acceptance | ⏳ pending | needs QEMU+hwsim and real hardware; the drill procedure is described in [`BUILDING_zh-cn.md`](BUILDING_zh-cn.md) |

Code size: **all 101 unit tests pass** (`wifisync-core` 64, `wifisync-sys` 23, `wifisync` 14), `clippy -D warnings` is clean, `cargo fmt --check` passes.

### 15.2 Deviations from the initial plan (and why)

| Original plan | Actual implementation | Reason |
|---------------|-----------------------|--------|
| tokio async runtime | **std threads + blocking I/O** | the service model is simple enough; this removes a huge runtime and makes the binary smaller and musl more stable |
| rtnetlink for bridge setup | **uci + ubus/netifd** | the idiomatic OpenWrt way, which naturally handles DSA/swconfig differences |
| httparse for the local API | **one JSON per line over a UNIX socket** | one less protocol surface and two fewer dependencies |
| static linking (`+crt-static`) | **dynamic linking against the device's musl** (`-crt-static`) | matches the official `rust-values.mk` and yields smaller binaries; the static option is still available outside `[profile.release]` |
| detect the full wpad via the package manager | **search for `mobility_domain` + `ieee80211r` in the hostapd/wpad binary** | opkg (ipk) and apk use different database formats, while the binary signature works on both |
| the plan did not mention `roles_configured` | a new uci option was added | requirement 3 asks to drop AP from the **default settings** when there is no wireless: at install time the hardware is unknown, so the first start computes and persists the defaults from the capabilities. **Wireless presence is authoritative via `/sys/class/ieee80211/*`**; leftover uci configuration does not count (covered by a regression test) |
| `wifisyncctl` as a separate binary | merged into **a single `wifisync` binary** (daemon/ctl/ubus modes) | better for package size and packaging complexity; the rpcd bridge is therefore just one `exec` line |
| "retry once → fall back to /rom" | `network restart` fallback is implemented; the `/rom` fallback and retry are left for M6 validation on real hardware | the timing of the restore path (procd stop order) must be validated on real hardware |

### 15.3 Next steps (M6)

1. QEMU (x86/64 + `kmod-mac80211-hwsim`) three-node topology: Gateway / Controller / pure AP, to
   validate "non-AP zero changes" against a real netifd;
2. validate the `pre_stop` restore timing on real hardware (procd sends SIGTERM → the daemon
   restores synchronously → `stop_service` performs one more idempotent restore);
3. land the Controller ↔ AP heartbeat message protocol (the `seal`/`open` helpers in `secrets.rs`
   are ready but not wired up yet);
4. if MIPS targets are needed, evaluate alternatives to `ring` (the project currently only
   commits to x86 / ARM64).
