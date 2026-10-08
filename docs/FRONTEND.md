**English** | [简体中文](FRONTEND_zh-cn.md)

# Frontend (LuCI) Design

> Scope: `luci/luci-app-wifisync`. The UI is a **pure web interface**: it never applies network
> configuration itself — every action goes through the `wifisync` backend.

Backend behavior, roles, backup/restore and failover are documented in
[`BACKEND.md`](BACKEND.md); building and packaging in [`BUILDING.md`](BUILDING.md).

---

## 1. Design constraints

1. **No direct network configuration.** LuCI must not contain any code that syncs network
   configuration; all functionality is handled by the backend service (`wifisync`) — see
   `AGENTS.md`.
2. **Zero intrusion must be visible.** Pages that could change the local network state up front
   whether they write anything, and risky actions require explicit confirmation and a dry-run
   preview.
3. **No new protocol.** The UI reuses standard LuCI mechanisms: ubus + `rpc.declare()`.
4. **No hard-coded language.** UI strings must go through LuCI's translation mechanism and be
   shipped as language packages that are loaded dynamically from the user's language preference,
   instead of being baked into the view code.

---

## 2. Communication contract

```
LuCI JS view  →  ubus object `wifisync`  →  rpcd exec plugin (/usr/libexec/rpcd/wifisync)
              →  UNIX socket /var/run/wifisync/wifisync.sock (0600)  →  wifisync daemon
```

* Views only call `rpc.declare({ object: 'wifisync', method: '…' })`; they never read or write
  `/etc/config` themselves.
* Access is granted by the ACL file `root/usr/share/rpcd/acl.d/luci-app-wifisync.json`, which
  separates **read** and **write** methods:

| Access | Methods |
|--------|---------|
| read | `status`, `capabilities`, `roles_get`, `bridge_preview`, `plan_dry_run`, `wifi_source_get`, `admission_list`, `backup_list`, `backup_verify`, `failsafe_get`, `profile_get`, `link_get`, `link_status`, `account_list`, `lan_report`, `lan_list`, `logs_tail`, `version` |
| write | `roles_set`, `apply`, `confirm`, `revert_last_change`, `restore`, `wifi_source_set`, `gateway_set`, `probe_connectivity`, `admission_register`, `admission_approve`, `admission_reject`, `admission_revoke`, `backup_create`, `backup_prune`, `failsafe_set`, `profile_publish`, `link_set`, `account_add`, `account_passwd`, `account_remove` |

Both sections also grant `uci: [ "wifisync" ]` (the service's own configuration only).

---

## 3. File layout

```
luci/luci-app-wifisync/
├─ Makefile                                  # luci.mk based; LUCI_DEPENDS := +wifisync +luci-base
├─ htdocs/luci-static/resources/view/wifisync/
│  ├─ common.js        # every rpc.declare() declaration, the MESSAGES table and shared widgets
│  ├─ overview.js      # status overview
│  ├─ roles.js         # role wizard
│  ├─ gateway.js       # LAN interface selection (zero intrusion)
│  ├─ controller.js    # admission list, Wi-Fi source, connectivity
│  ├─ bridge.js        # bridge planning preview (pure AP only)
│  ├─ wifi.js          # Wi-Fi & KVR
│  ├─ backup.js        # backup, restore and failover
│  └─ diagnostics.js   # logs, dry-run, tarball
├─ po/zh_Hans/luci-app-wifisync.po           # translation catalog (luci.mk builds luci-i18n-wifisync-zh-cn)
└─ root/usr/share/
   ├─ luci/menu.d/luci-app-wifisync.json     # menu registration (`admin/services/wifisync`, ACL gated)
   └─ rpcd/acl.d/luci-app-wifisync.json      # read/write ubus allow-lists
```

`common.js` is the single place where the ubus method surface is declared, so a backend method
rename only has to be done once.

---

## 4. Pages

Menu group: **Services → WifiSync** (`admin/services/wifisync`).

| Order | Page | Content |
|-------|------|---------|
| 10 | Overview | Role badges, wireless presence, **Controller↔Gateway connectivity badge**, **pending admission count**, bridge members, heartbeat/rollback countdown, **baseline status and last restore time** |
| 20 | Roles | Three checkboxes; AP is disabled with a hint when there is no wireless; an explicit note that "Gateway/Controller will not modify your network configuration"; **Link to the Controller** (address, account, password, link state — the password is write-only) |
| 30 | Gateway | Only "select the corresponding LAN interface" + probe target; **the LAN information the Gateway reports to the Controller** (bridge, network, DHCP range, IPv6 policy, read-only); a permanent zero-intrusion note at the top; **no network-writing controls at all** |
| 40 | Controller | ① **admission list** (pending/approved/blacklist, approve/reject/revoke) ② **Wi-Fi source choice** (self/Gateway/custom, with the reason when disabled) ③ `NetworkProfile` preview and publish button ④ connectivity probe target and "confirm connectivity" ⑤ **listener** (listen address and port, 6550 by default) ⑥ **accounts** for APs and Gateways (create / change password / delete) ⑦ **LAN information reported by the Gateways** |
| 50 | Bridge | **Visible to pure AP devices only**; automatic plan preview + dry-run diff; for other role combinations it shows "the current role combination creates no bridge" and why; the reserved multi-bridge/VLAN area is collapsed by default |
| 60 | Wi-Fi & KVR | `mobility_domain`, FT mode, k/v switches; `wpad` version check and install guidance |
| 70 | Backup & failover | Baseline info (time / checksum / `managed_keys`), snapshot lists (pre-start / pre-change), integrity verification, `restore_mode` selection, **"restore baseline now"**, **"re-baseline"** (double confirmation + dry-run preview), restore-on-stop escape hatch, plus the failover switches, timeouts, action and manual rollback (AP only, noted on the page) |
| 80 | Diagnostics | Log tail, one-click dry-run comparison, sync version and diff, **backup integrity report** |

> The LuCI page list is the front-end counterpart of requirement R1; the UI-specific slices of R3,
> R5, R6, R9 and R11 (disabling, visibility, confirmations) are described in §5 and §6.

---

## 5. Interaction and safety patterns

| Pattern | Where | Behavior |
|---------|-------|----------|
| Zero-intrusion banner | Overview, Roles, Gateway, Bridge | States that the Gateway/Controller roles produce no local network writes; the Bridge page explains that only a pure AP creates a bridge |
| Disabled with reason | Wi-Fi source selectors, AP checkbox, Bridge page | Controls are greyed out with the reason (e.g. "the Gateway reports no Wi-Fi", "this device has no wireless") instead of silently disappearing |
| Dry-run preview | Overview (`plan_dry_run`), Bridge (`bridge_preview`) | Shows the uci/network diff before anything is written; for non-AP roles the preview is always empty |
| Apply + confirm | Overview | `apply` arms the apply-guard dead-man timer; the user must click `confirm` within the timeout or the change is rolled back |
| Double confirmation | same-device `custom` Wi-Fi, `full` restore, re-baseline, no-restore-on-stop | An extra explicit dialog plus a preview before the action |
| Badges | Overview, Controller, Backup | Connectivity "unconfirmed / confirmed", pending admissions, baseline validity, last restore time |

---

## 6. Role and capability gating in the UI

| Condition | Visible / enabled |
|-----------|-------------------|
| No wireless (`/sys/class/ieee80211/*` empty) | AP checkbox disabled; default roles `controller + gateway`; Bridge page shows "no bridge" |
| Not a pure AP | Bridge page shows the empty plan and the reason; no per-port controls |
| Pure AP (`ap && !gateway && !controller`) | Bridge page shows the planned port list and the dry-run diff |
| Controller role | Admission list, Wi-Fi source selection, profile publish, connectivity confirmation |
| Gateway role | LAN interface selection and probe target only |
| AP role | Wi-Fi/KVR page and the apply/confirm + revert controls are meaningful; backup/restore is available on every device |

---

## 7. i18n (language packages)

* Language must not be hard-coded: every user-visible string in the views is wrapped in LuCI's
  `_()` translation helper, with the English text as the msgid (English is the source language).
* Translations are shipped as separate LuCI language packages. The catalog lives in
  `po/<lang>/luci-app-wifisync.po`; `luci.mk` discovers `po/*` automatically and builds a
  `luci-i18n-wifisync-<lang>` package (e.g. `po/zh_Hans/` → `luci-i18n-wifisync-zh-cn`), which
  installs `/usr/lib/lua/luci/i18n/luci-app-wifisync.<lang>.lmo`. The catalog is selected through
  the `luci.languages` setting, so the UI follows the user's language preference without
  rebuilding the application.
* `menu.d` titles carry the English source string as well and are resolved by LuCI through the
  same catalog.
* Current status: `zh_Hans` is provided; adding another language only requires a new
  `po/<lang>/luci-app-wifisync.po` file.
* **Backend text is not pre-rendered either**: hint fields such as `plan.notes`,
  `bridge_blocked_reason`, `failsafe.state_label`, the Wi-Fi source reasons, role adjustments,
  backup/restore summaries, apply/gateway messages and probe details arrive as structured
  messages `{ key, params }` (see `wifisync-core::message`). `common.js` holds the `MESSAGES`
  table mapping every key to an `_('English template with %{param}')` string, and
  `ws.message()`, `ws.planText()` and `ws.errorText()` render them. Both key sets must be kept in
  sync by hand when either side changes (the comparison is a review step, not a CI job).
* The CLI and the service log are **plain English** and never translated — localization only
  happens here in the views, so the diagnostics page shows English log lines (see `AGENTS.md`).

---

## 8. Acceptance and testing

* **Static checks (review)**: run `node --check` on every view file and validate `menu.d` /
  `acl.d` as JSON before submitting a front-end change.
* **Translation coverage**: every `_('…')` string in the views must have a matching `msgid` in
  each `po/*` catalog (the `zh_Hans` catalog is exhaustive; menu titles are extracted from
  `menu.d`).
* **Message key consistency**: every key emitted by the backend (`Message::new("…")`) must exist
  in the `MESSAGES` table of `common.js` and vice versa (compared during review).
* **ACL walkthrough**: `ubus -v list wifisync` must match the declarations in `common.js`, and
  each method must fall into the correct read/write group in the ACL file.
* **Manual/browser regression**: every page loads under `admin/services/wifisync`; a non-AP role
  shows an empty dry-run; the admission approve/reject flow works; backup verify/restore works;
  the Bridge page is hidden/empty for non-AP combinations.
* **Packaging**: `release.yml` builds `luci-app-wifisync` with the official SDK for OpenWrt 24.10
  (`.ipk`) and 25.12 (`.apk`), which validates the feed package layout.

---

## 9. Front-end specific risks

| Risk | Mitigation |
|------|------------|
| Users expect the Gateway/Controller to configure the network automatically | permanent zero-intrusion text, empty dry-run as evidence, explicit confirmation badges |
| A risky write cuts the device off | dry-run preview + apply-guard countdown + one-click revert, plus the double-confirmation dialogs |
| Greying out controls without explanation confuses users | always render the disable **reason** next to the control |
| Front-end/back-end method drift | all `rpc.declare()` calls live in `common.js`, and the ACL file is reviewed together with the rpcd method table |
| Log lines shown on the diagnostics page are English only | accepted trade-off: CLI and service output must stay plain English (see `AGENTS.md`), while the UI exposes translated status fields |
