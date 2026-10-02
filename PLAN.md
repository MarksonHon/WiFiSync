# WifiSync — OpenWrt 家庭局域网（含有线+无线）组网工具 · 实施计划表

> 语言：Rust（musl 静态）/ 界面：LuCI / 平台：OpenWrt 22.03 ~ 24.10+（musl 目标）
> 定位：**不实现 mesh**；用「Controller 准入 + 下发网络信息 + 802.11k/v/r 漫游 + 标准 Wi-Fi 中继」组成家庭单网。
> 核心原则：**默认零侵入** —— 只有「纯 AP」设备才会被程序修改网络；Gateway/Controller 只读、只探测、只下发。
> 生命周期纪律：**启动先备份初始化网络，停止先恢复原有网络**。

---

## 1. 需求追溯表（Requirements → Design → 验收）

| # | 需求 | 设计要点 | 落地模块 | 验收方式 |
|---|------|---------|---------|---------|
| R1 | 包含 LuCI 界面 | 守护进程只提供本地 Unix Socket JSON API → `rpcd` exec 插件桥接为 **ubus 对象** → LuCI JS 用 `rpc.declare()` 调用（零新协议） | `luci/luci-app-wifisync/`、`files/usr/libexec/rpcd/wifisync` | ubus `list/call` 走查 + 浏览器功能回归 |
| R2 | 不使用 musl 编不过的库 | 依赖白名单 + 纯 Rust 密码学 + 静态链接 musl，见 §4 | 全工程、`.cargo/config.toml` | 5 个 musl target 编译通过 + 单文件二进制 ≤ 3 MB |
| R3 | 角色 Controller/AP/Gateway；无无线时禁用 AP | 角色 = **能力集合**；无无线 ⇒ 默认角色去掉 AP 且 UI 置灰禁用 | `core::role`、`sys::capability` | 无无线设备默认 `{controller,gateway}`；AP 复选框禁用 |
| R4 | 支持 KVR，不支持 mesh；中继走标准 Wi-Fi 中继 | 生成 hostapd/wpa_supplicant KVR 参数；中继 = `wpa_supplicant(STA)` + `hostapd(AP)` (+relayd)，**无 802.11s** | `core::kvr`、`core::relay` | hwsim 双 AP 漫游日志/切换时延；代码中无 mesh 分支 |
| R5 | ⚠️ 已收窄：**仅「纯 AP」**才把所有网口组进 `br-lan` | 建桥条件 = `ap && !gateway && !controller`；其它任何角色组合一律 **不建桥、不动网口** | `core::bridge`、`sys::netlink` | 纯 AP 场景全部网口入 `br-lan`；`ap+controller`/`ap+gateway`/`controller+gateway` 场景 dry-run diff 为空 |
| R6 | 可选：连不上 Controller/Gateway 时恢复默认网络 | 心跳 + 死手定时器 + 回滚引擎（三级护栏），默认关闭；**只作用于 AP 设备** | `core::failsafe`、`sys::rollback` | 断链超时后自动恢复出厂网络配置并回到可用状态 |
| R7 | Gateway 默认**不代替用户改任何网络**，只要求选 LAN 接口 | Gateway = 只读标识角色：用户在 UI 选定「对应 LAN 接口」用于拓扑/告警；写入计划的配置项恒为空 | `core::role::gateway`、`core::uplink` | 选定 LAN 口后，本机 uci/network 的 diff 恒为 **0 字节** |
| R8 | Controller 默认只做「新 AP 验证」+「网络信息下发」，不改现有网络 | Controller 不写本机网络配置；与 Gateway 的连通性由**用户单独确认**（程序只做探测，不改路由/NAT/防火墙） | `core::admission`、`core::distribution`、`core::probe` | 准入列表可批准/拒绝；连通性检测实现为只读探针 + 显式确认徽标 |
| R9 | AP 默认同步 Controller 下发的网络信息（含 Wi-Fi）；Wi-Fi 信息源三选一 | 来源：`controller_self` / `gateway` / `custom`；来源设备报告无 Wi-Fi ⇒ 该来源禁用；选 `custom` 且 AP 与 Controller **同设备** ⇒ 该设备 Wi-Fi 一并被改 | `core::wifi_source`、`core::profile` | 三来源切换与禁用逻辑正确；同设备 `custom` 场景生成正确无线配置（含二次确认） |
| R10 | 不写死：预留多网桥 / VLAN 跨设备网桥同步 | 计划结构一律为**列表**（`Vec<BridgePlan>`、`vlans[]`、`bridges[]`），默认只产出一个 `br-lan` | `core::bridge`、`core::vlan`、`core::profile` | 单测覆盖多网桥 + VLAN tagged/untagged 关联；无硬编码单网桥分支 |
| R11 | **服务启动前必须备份一次初始化网络参数；服务停止前必须恢复原有网络** | 生命周期钩子：`pre_start` → 首次创建**不可变初始基线** `initial/`（+ 每次启动滚动快照）；`pre_stop` → 按基线**同步恢复**后再退出；含脏标记检测、完整性校验、`managed_only`/`full` 两种恢复范围 | `core::backup`、`sys::snapshot`、`sys::restore`、`etc/init.d/wifisync` | 启停各一轮后网络配置与初始基线 diff=0；`kill -9` 后下次启动能识别脏状态并提示/自动恢复 |

**贯穿性原则（本版新增）**

| 原则 | 含义 |
|------|------|
| 零侵入 | Gateway / Controller 角色自身**不产生任何网络配置写入**；只有 `ap` 角色会写（且必须经准入 + 二次确认 + apply-guard） |
| 显式确认 | 连通性、同设备改 Wi-Fi 等有风险动作，一律由用户显式点击确认，程序只提供探测与预览 |
| 最小职责 | Controller = 验证 + 下发；Gateway = 标识 + 探测端点；AP = 应用 + 上报 |
| 可回退 | **先备份后改动，先恢复再退出**：任何网络写入前必有基线快照；服务停止前必须还原到初始网络（见 §10） |

---

## 2. 里程碑计划表

| 阶段 | 目标 | 主要任务 | 交付物 | 验收标准 | 预估 |
|------|------|---------|-------|---------|------|
| **M0** 可行性验证 | 打通三处技术风险 | ① musl 静态交叉编译 PoC 与体积测量 ② netlink 建桥/加端口 PoC ③ rpcd shim + LuCI 空页面跑通 ④ hostapd KVR 参数在 `mac80211_hwsim` 下生效 | `docs/spike-*.md`、可运行 PoC | 4 项 PoC 全部在 QEMU OpenWrt x86/64 上跑通 | 3–4 天 |
| **M1** 核心逻辑库 | 纯逻辑、可单测、无系统调用 | 配置模型(UCI/JSON 双向)、角色决策、能力探测抽象、**建桥策略(仅纯 AP)+多网桥/VLAN 列表模型**、KVR 规划器、中继规划器、**Wi-Fi 信息源解析器**、`NetworkProfile` 版本与合并、**准入状态机**、failsafe 状态机 | `crates/wifisync-core` + 单测 | 覆盖率关键分支 100%；`cargo test` 全绿（host 端） | 6–8 天 |
| **M2** 系统适配层 | 只读探测为主 + 仅 AP 侧写入 | 只读：uci/netifd/netlink/iwinfo/nl80211 能力与拓扑读取、**Gateway「LAN 接口」选择落地（仅记录，不修改）**、连通性探针；**快照与恢复落地：`uci export` 全量 + `iw`/`ip -j` 转储 + sha256 清单 + 原子写 + 恢复执行器**；写入（仅 AP）：staging/commit、netifd reload、rtnetlink 建桥/从属、hostapd/wpa_supplicant 渲染、`wpad` 检查；VLAN 可用性探测 | `crates/wifisync-sys` | 非 AP 角色 dry-run diff 恒为 0；AP 侧完成「准入→下发→应用→连通」闭环；**快照/恢复可独立跑通（人工篡改可被校验发现）** | 7–9 天 |
| **M3** 守护进程与安全护栏 | 长驻服务 + 准入 + 下发 + 回滚 | Tokio 服务、Unix Socket JSON-RPC、设备心跳(PSK-HMAC)、**准入服务(pending/approve/reject)**、**Profile 下发与版本协商**、apply-guard 死手定时器、link-watchdog(仅 AP)、`revert` 引擎、procd 自愈、**生命周期钩子（pre_start 备份 / pre_stop 恢复 / SIGTERM 同步收尾 / 脏标记）** | `crates/wifisync-daemon`、`etc/init.d/wifisync` | AP 被拒绝/未准入时不改本机配置；AP 断链 N 秒后恢复默认网络并可重新加入；**启停一轮后网络回到初始状态** | 8–10 天 |
| **M4** LuCI 应用 | 完整可视化配置 | 状态总览、角色向导、**Gateway(LAN 口选择，注明零侵入)**、**Controller(准入列表 + Wi-Fi 来源三选一)**、**连通性确认**、网桥(仅纯 AP 可见)、KVR、中继、failsafe、**备份与恢复**、诊断/日志；ACL 与菜单注册 | `luci/luci-app-wifisync` | 全流程无需 CLI 即可完成组网；可查看/校验/手动恢复初始基线 | 8–10 天 |
| **M5** 打包与发布 | 可安装 ipk/apk + 多架构 | OpenWrt package Makefile、init.d、默认 uci 配置、rpcd ACL、CI 多 target 构建、`DEPENDS`=wpad/relayd/kmod-* | `openwrt/package/wifisync/`、CI 产物 | 官方 SDK 编译出包，opkg/apk 安装即用 | 4–5 天 |
| **M6** 联调与验收 | 真实拓扑验证 | QEMU+hwsim 三节点（GW/Controller/纯AP）、中继场景、漫游时延测量、**非侵入性回归（校验非 AP 设备 diff=0）**、**启停循环回归（start→stop 后配置归零）**、断电/断链回归、真机抽测 | 测试报告 | R1–R11 全部验收通过 | 7–9 天 |

---

## 3. 角色模型

| 角色 | 默认 | 职责（收窄后） | 对本机既有网络的改动 | 建桥 |
|------|------|--------------|-------------------|------|
| `gateway` | 有无线时默认开 | 仅**标识**：用户在 UI 选定「对应 LAN 接口」，供拓扑展示与告警；提供连通性探测端点 | **无**（不写 uci、不调 netifd、不动 NAT/防火墙） | 不建桥 |
| `controller` | 有无线时默认开 | ① **新 AP 验证（准入）** ② **网络信息下发（含 Wi-Fi）** ③ 心跳汇聚与拓扑展示 | **无**（不改本机网络；与 Gateway 是否连通由**用户单独确认**） | 不建桥 |
| `ap` | 有无线时默认开；**无无线时强制关闭** | 同步 Controller 下发信息并应用（Wi-Fi/漫游域）、上报状态 | 仅应用下发的无线/网桥信息，且**必须已通过准入 + 二次确认** | **仅纯 AP 建桥** |

**建桥策略（唯一条件）**

```
enable_bridge = roles.ap && !roles.gateway && !roles.controller
```

**角色决策矩阵**

| 条件 | 默认角色集合 | 本机配置写入 | 网桥策略 |
|------|-------------|-------------|---------|
| 有无线 + 主路由设备 | `controller + gateway + ap` | 仅 Wi-Fi（来自 Controller 下发/所选来源） | 不建桥 |
| 有无线 + 纯 AP | `ap` | Wi-Fi + 建桥 | 全部网口 → `br-lan` |
| 有无线 + AP 与 Controller 同设备 | `controller + ap` | 仅 Wi-Fi（若是 `custom` 来源则含本机 Wi-Fi） | 不建桥 |
| 有无线 + AP 与 Gateway 同设备 | `gateway + ap` | 仅 Wi-Fi | 不建桥 |
| 无无线 + 主路由 | `controller + gateway`（AP 置灰禁用） | **无** | 不建桥（仅记录用户选定的 LAN 接口） |
| 无无线 + 纯控制器 | `controller` | **无** | 不建桥 |

---

## 4. 依赖选型（musl 约束）

| 用途 | 选用 | 规避/理由 |
|------|------|----------|
| 运行时 | **std 线程 + 阻塞 IO**（不引入 `tokio`） | 服务只需 UNIX socket + 定时器 + 线程，std 足够，省掉一个巨大运行时 |
| 序列化 | `serde` + `serde_json` | 纯 Rust |
| 网络配置 | **uci + ubus/netifd**（不引入 `rtnetlink`） | 网桥落地写 uci `config device type bridge` 再 `ubus call network reload`，这才是 OpenWrt 惯用做法，也免去自己处理 DSA/swconfig 差异 |
| 状态读取 | `sysfs` + `/etc/board.json` + `iwinfo` | 只读；不依赖 `ip -j`（busybox 的 ip 不支持 JSON） |
| 本地 API | UNIX socket 上的「一行一个 JSON」协议（不引入 `httparse`/`hyper`/`axum`） | 少一层协议解析面、少几个依赖；socket 权限 0600 做隔离 |
| 节点间认证 | `hmac` + `sha2` + `chacha20poly1305` | 纯 Rust，全架构可用（含 MIPS） |
| 信号 | `libc`（仅 `sigaction`） | SIGTERM/SIGINT/SIGHUP，其他一律走 std |
| 错误 | `thiserror` | 编译期宏，无运行时开销 |
| 凭据/状态存储 | JSON 文件 + 原子写（`/etc/wifisync/`，0600） | 避免 SQLite 体积与 musl 交叉麻烦 |
| CLI 解析 | 手写（`main.rs` 里的 `match`） | 命令集固定，省掉依赖 |
| ❌ 禁止 | `openssl-sys`、`ring`、`aws-lc-rs`、`git2`、`libz-sys(非 bundled)`、任何需 `cmake` 的 crate | musl/交叉/体积不可靠；**ring 不支持 MIPS/MIPSEL** |
| ⚠️ 慎用 | `regex`、`libsqlite3-sys`、`tokio` 全特性 | 显著增大二进制 |

**构建与体积**

| 项 | 措施 |
|----|------|
| 目标三元组 | `x86_64` / `i686` / `i586` / `aarch64` `-unknown-linux-musl`（覆盖 OpenWrt 25.12 全部 x86 与 ARM64 包架构，见 §12） |
| 链接 | 用 **OpenWrt 官方 SDK** 的 `*-musl-gcc`；`-C target-feature=-crt-static`（动态链接设备上的 musl，与官方 `rust-values.mk` 一致） |
| 体积 | `opt-level="z"`、`lto=true`、`codegen-units=1`、`panic="abort"`、`strip`；CI 门禁 **3 MiB** |
| 复现 | 提交 `Cargo.lock`，构建一律 `--locked` |

---

## 5. KVR 与中继设计

| 能力 | hostapd（AP 侧） | wpa_supplicant（STA 侧） | 说明 |
|------|-----------------|------------------------|------|
| 802.11k | `ieee80211k=1`, `rrm_neighbor_report=1` | `rrm_neighbor_report=1` | 邻居报告 |
| 802.11v | `ieee80211v=1`, `bss_transition=1` | `bss_transition=1` | 引导客户端切换 |
| 802.11r | `ieee80211r=1`, `mobility_domain=`, `ft_over_ds=1`, `ft_psk_generate_local=1`, `r0kh`/`r1kh`/`nasid` | `ieee80211r=1`, `mobility_domain=`, `ft_psk_generate_local=1` | 快速漫游，FT 走 DS |

- 依赖 **`wpad`（完整版）**，不是 `wpad-basic`；启动前做完整性检查并给出安装提示。
- 全部 KVR 参数**不是本地硬编码**，而是随 `NetworkProfile.wifi[].kvr` 由 Controller 的 Wi-Fi 信息源（见 §6）统一下发；`mobility_domain` 全局唯一，保证跨 AP 快速漫游。
- AP 应用 Wi-Fi 配置前必须已通过准入（见 §7）。
- **中继（无 mesh）**：`wpa_supplicant(STA)` 上行 + `hostapd(AP)` 下行；二层透明用 `relayd`，或路由模式；可选 4-address/WDS。中继链路下 802.11r 受限时回退 `ft_over_ds`。

---

## 6. Wi-Fi 信息源与网络信息同步（R9）

**Controller 的 Wi-Fi 信息来源三选一**

| 来源 | 取值方式 | UI 禁用条件 | 对设备的影响 |
|------|---------|------------|-------------|
| `controller_self` | 读取 Controller 本机 uci `wireless` + iwinfo 实际状态 | Controller 上报「无 Wi-Fi」时禁用 | 不改 Controller 本机 Wi-Fi（只作为模板下发给 AP） |
| `gateway` | 只读拉取 Gateway 的 Wi-Fi 档案（地址 + 凭据由用户填写；**绝不写回 Gateway**） | Gateway 上报「无 Wi-Fi」/ 未选定 / 拉取失败时禁用 | 不改 Gateway |
| `custom` | 用户在 LuCI 手工定义 SSID、加密、PSK、频段/信道、带宽、KVR、mobility_domain | 无（始终可用） | ⚠️ 若 AP 与 Controller **同设备**，该设备 Wi-Fi 也会按 `custom` 被修改（需二次确认 + dry-run 预览 + apply-guard） |

- 禁用为**双重校验**：LuCI 端置灰 + daemon 端拒绝（防绕过）。
- 只有在「Controller 与 AP 同设备」且来源为 `custom` 时，本机 Wi-Fi 才进入写入计划；其余情况 Controller 侧 diff 恒为 0。
- 三来源的优先级/回退可配：如 `gateway` 拉取失败时是否回退 `controller_self`（默认不自动回退，仅告警）。

**下发内容：`NetworkProfile`（列表化，可扩展）**

```jsonc
{
  "version": 42,                       // 单调递增，冲突时高版本胜
  "bridges": [                         // 列表 → 未来可多网桥
    { "name": "br-lan", "ports": ["lan1","lan2"], "vlan_filtering": false, "vlans": [] }
  ],
  "vlans": [                           // 预留：VLAN 跨设备网桥同步
    { "id": 10, "name": "iot", "tagged_ports": [], "untagged_ports": ["lan3"] }
  ],
  "wifi": [                            // 每 radio 一个 profile
    { "radio": "radio0", "ssid": "Home", "auth": "sae-mixed", "psk_ref": "…",
      "band": "5g", "channel": 44, "kvr": { "k": true, "v": true, "r": true,
      "mobility_domain": "abcd", "ft_over_ds": true } }
  ]
}
```

**AP 同步行为**

| 项 | 默认 | 说明 |
|----|------|------|
| 同步开关 | `sync_mode = auto`（跟随 Controller） | 可切 `local_override`（本地临时覆盖，UI 显著警示，Controller 版本更新不覆盖本地） |
| 同步内容 | 网络信息（网桥/VLAN 定义）+ Wi-Fi 信息（含 KVR/漫游域） | Wi-Fi 为必同步项 |
| 生效时机 | 版本号变化时 | 应用前 dry-run 预览 + apply-guard |
| 断链 | 保持最后一次成功配置 | 由 failsafe 决定是否恢复默认（见 §9） |

---

## 7. Controller 准入与连通性确认（R8）

**新 AP 准入流程（Controller 的唯一"主动"职责之一）**

1. AP 侧安装后启动 → 上报 `device_id`(持久化 UUID) + MAC + 无线能力 + 本地凭据签名。
2. Controller 记为 `pending`，LuCI「准入列表」显示（含来源 IP、MAC、时间、无线能力）。
3. 管理员**显式**点击「批准」→ Controller 下发 `NetworkProfile`；点击「拒绝」→ 记入黑名单，AP 永不写入本机配置。
4. 凭据：预共享密钥 + 心跳 HMAC-SHA256（配合 `chacha20poly1305` 加密载荷），**不使用 TLS**（见 §4 依赖约束）。

**Controller ↔ Gateway 连通性（用户单独确认）**

| 项 | 设计 |
|----|------|
| 探测方式 | 只读探针：ICMP echo / TCP 端口可达 / HTTP 探针字节；可配目标（Gateway 地址或任意检测点） |
| 是否改动网络 | **否** —— 不写路由表、不改防火墙、不动 NAT |
| 用户确认 | UI 显示「未确认 / 已确认」徽标，需用户显式点击确认；确认状态写入 wifisync 自有 uci |
| 失败提示 | 仅告警 + 给出排查建议（不自动修复） |

> Controller 不参与数据面转发；Gateway 的 LAN 接口选择仅用于拓扑标识与探针目标推断。

---

## 8. 网桥规划器（收窄 + 可扩展，R5 / R10）

**启用条件（默认策略，可被未来策略覆盖）**

```
enable_bridge = roles.ap && !roles.gateway && !roles.controller    // 仅「纯 AP」
```

1. 非纯 AP ⇒ 规划器返回**空计划**（no-op），不产生任何 uci/网络写入。
2. 纯 AP ⇒ 端口枚举（优先 DSA：`/sys/class/net/*/dsa`、`board.json`；回退 `swconfig`）→ 全部网口（含原 WAN 口）并入 `br-lan`。
3. 输出结构一律为 `Vec<BridgePlan>` + 每桥 `vlan_filtering` / `vlans[]`；默认只生成一个 `br-lan`，`bridge.name` 可配，**代码中不出现单网桥假设**。
4. Gateway 场景：仅记录用户选定的 LAN 接口（`uplink.lan_ifaces`），不建桥、不移除 WAN 从属。
5. 落地：写 uci `config device type bridge` + `netifd reload`，并用 rtnetlink 校验实际链路。
6. 变更前快照 uci → 武装 apply-guard（见 §9）。
7. **VLAN 预留**：`VlanDef{id, name, tagged_ports, untagged_ports}` 已存在于 profile，本版默认 `vlans: []`；跨设备网桥同步只需扩展传输与合并逻辑，无需改模型。

---

## 9. 故障恢复（R6，可选，默认关闭，只作用于 AP）

| 层级 | 触发 | 动作 |
|------|------|------|
| L1 apply-guard | 应用下发配置后未在 `confirm_timeout` 内确认连通 | 自动回滚 uci 快照 + `network restart` |
| L2 link-watchdog | **AP** 与 Controller/Gateway 心跳丢失 > `failsafe_timeout` | 恢复网络：优先用 §10 的**初始基线**还原；基线不可用时回退 `/rom/etc/config/network` + 重启网络；可选 `reboot -f` |
| L3 兜底自愈 | 守护进程崩溃 | procd `respawn` + 启动自检；24h 内反复失败进入安全模式 |

- Gateway / Controller **不触发**网络回滚（零侵入原则），仅告警。
- 配置项：`failsafe.enabled`(默认 `0`)、`failsafe_timeout`(默认 `300s`)、`failsafe_action`(`revert`|`reboot`)、`failsafe_keep_ssid`、心跳端点与 PSK。

---

## 10. 备份与恢复生命周期（R11）

**核心规则：先备份，后改动；先恢复，再退出。**

| 时机 | 动作 | 说明 |
|------|------|------|
| `pre_start`（服务启动前，先于任何探测/写入） | ① 若不存在初始基线 → 创建 `initial/`（**不可变**）② 若存在 → 校验完整性；③ 每次启动追加滚动快照 `pre-start-<ts>/`（保留最近 N 份） | 基线一旦建立不再被覆盖，除非用户显式「重新基线」 |
| `running` | 任何写入（Wi-Fi/网桥/下发应用）前再拍一张 `pre-change-<ts>/` | 与 apply-guard 配合，供 L1 回滚 |
| `pre_stop`（服务停止前，**同步、阻塞、可超时**） | 按基线**恢复原有网络** → `uci commit` → `network restart` / `wifi reload` → 校验恢复结果 → 清理运行时脏标记 → 才允许退出 | 由 procd `stop_service`、`SIGTERM`、`opkg` prerm、关机流程共同触发 |
| 异常退出后下次启动 | 检测脏标记 ⇒ 提示（可配自动）先恢复基线再启动 | `kill -9` / 断电场景的兜底 |

**备份内容（全部为文本，便于 diff 与压缩）**

| 项 | 采集方式 |
|----|---------|
| uci 配置全量 | `uci export network/wireless/dhcp/firewall/system`（逐文件保存，保留原始 `/etc/config/*` 副本） |
| 无线实际状态 | `iwinfo` / `iw dev` / `iw dev <if> info` 输出 |
| 链路状态 | `ip -j link`、`ip -j addr`、`ip -j route`（bridge/brport 从属关系） |
| 元数据 | `manifest.json`：时间、device_id、当前角色集合、wifisync 版本、**每文件 sha256**、`managed_keys`（本次由 wifisync 改动的 uci 键路径清单） |

```
/etc/wifisync/backup/
├─ initial/                  # 不可变初始基线（权威还原源）
│  ├─ manifest.json
│  └─ config/{network,wireless,dhcp,firewall,system}
├─ pre-start-<ts>/           # 每次启动滚动快照（保留 5 份）
└─ pre-change-<ts>/          # 每次写入前快照（保留 5 份）
```

**恢复范围与安全策略**

| 项 | 默认 | 说明 |
|----|------|------|
| `restore_mode` | `managed_only` | 只还原 `managed_keys` 中由 wifisync 改过的键；用户自己新增/修改的无关配置保留。`full` = 按基线整体覆盖（UI 二次确认） |
| 幂等性 | 必满足 | 重复恢复结果一致；恢复前先校验 sha256，缺失/损坏则拒绝执行并告警 |
| 失败处理 | 重试 1 次 → 回退 `/rom` | 恢复失败不得删除基线 |
| 逃生开关 | `wifisyncctl stop --no-restore` | 用户明确要求保留当前配置时使用（UI 需二次确认） |
| 占用控制 | 仅文本 + 体积上限 | 超限时按策略裁剪滚动快照（`initial/` 永不裁剪） |

---

## 11. 工程结构

```
WifiSync/
├─ Cargo.toml                # workspace
├─ rust-toolchain.toml
├─ .cargo/config.toml        # 交叉 linker / 体积参数
├─ PLAN.md
├─ crates/
│  ├─ wifisync-core/         # 纯逻辑 + 单测
│  │   ├─ role              # 角色集合与建桥策略(仅纯 AP)
│  │   ├─ capability        # 无线/网口/VLAN 能力抽象
│  │   ├─ bridge            # BridgePlan(Vec) + VlanDef + 端口枚举（可扩展多网桥/VLAN）
│  │   ├─ wifi_source       # controller_self | gateway | custom 解析与禁用判定
│  │   ├─ profile           # NetworkProfile 版本、合并、下发 diff
│  │   ├─ admission         # 新 AP 准入状态机(pending/approved/rejected)
│  │   ├─ uplink            # Gateway「LAN 接口」选择模型（只读）
│  │   ├─ probe             # 连通性探针规格（只读）
│  │   ├─ backup            # 基线/快照模型、managed_keys、恢复计划、保留策略
│  │   ├─ kvr / relay       # KVR 与标准中继规划
│  │   └─ failsafe          # 死手定时器 + watchdog 状态机
│  ├─ wifisync-sys/          # uci/netifd/netlink/iwinfo/hostapd 适配（读为主，写仅 AP）
│  │   ├─ snapshot          # uci export + iw/ip -j 转储 + sha256 清单 + 原子写
│  │   └─ restore           # 基线还原执行器（managed_only / full）
│  ├─ wifisync-daemon/       # 服务：生命周期钩子 + API + 准入 + 下发 + 心跳 + watchdog + apply/rollback
│  └─ wifisyncctl/           # CLI（status/plan --dry-run/apply/revert/admit/join/backup/restore/stop --no-restore）
├─ openwrt/
│  ├─ package/wifisync/Makefile
│  └─ files/{etc/init.d,etc/config,usr/libexec/rpcd,usr/share/rpcd/acl.d}
├─ luci/luci-app-wifisync/   # luasrc/controller + htdocs JS view + menu.d + acl.d
├─ tests/{unit,integration,dryrun}
└─ scripts/{build-musl.sh,qemu-lab.sh,hwsim-lab.sh}
```

---

## 12. LuCI 页面清单

| 页面 | 内容 |
|------|------|
| 状态总览 | 角色徽标、无线存在性、**Controller↔Gateway 连通性徽标**、**待准入 AP 数**、网桥成员、心跳/回滚倒计时、**基线状态与最近一次恢复时间** |
| 角色向导 | 三复选框；无无线时 AP 禁用并提示；显式标注「Gateway/Controller 不会修改你的网络配置」 |
| **Gateway** | 仅「选择对应 LAN 接口」+ 探针目标；页面顶部常驻零侵入说明；无任何网络写入控件 |
| **Controller** | ① **准入列表**（待批准/已批准/黑名单，批准/拒绝/撤销）② **Wi-Fi 信息源三选一**（自身/Gateway/自定义，含禁用原因提示）③ `NetworkProfile` 预览与下发按钮 ④ 连通性探测目标与「确认连通」按钮 |
| 网桥 | **仅纯 AP 设备可见**；自动规划预览 + dry-run diff；非纯 AP 时显示「当前角色组合不建桥」及原因；预留多网桥/VLAN 高级区（默认折叠） |
| KVR | mobility_domain、FT 模式、k/v 开关；`wpad` 版本检查与安装引导 |
| 中继 | 上行 SSID/密码扫描与选择、下行 AP 参数、透明桥/路由模式 |
| 故障恢复 | 开关、超时、动作、当前快照列表、手动回滚（仅 AP 侧生效，页面注明） |
| **备份与恢复** | 初始基线信息（时间/校验和/`managed_keys`）、快照列表（pre-start / pre-change）、完整性校验按钮、`restore_mode` 选择、**「立即恢复基线」**、**「重新建立基线」**（二次确认 + dry-run 预览）、停止时是否恢复（逃生开关） |
| 诊断 | 生成 tarball、`iw`/`iwinfo`/hostapd 日志、一键 dry-run 对比、同步版本号与 diff、**备份完整性报告** |

---

## 13. 风险与对策

| 风险 | 影响 | 对策 |
|------|------|------|
| MIPS Tier3 目标需 nightly `build-std` | 构建复杂 | CI 固定 nightly + 缓存；M0 先验证 |
| 小 flash 设备放不下二进制 | 无法安装 | 体积门禁 ≤3MB；`-Z build-std` + `panic_immediate_abort`；必要时按角色裁剪 feature |
| AP 改网络把设备弄失联 | 用户断网 | 仅纯 AP 写入 + 准入 + 二次确认 + dry-run + L1 apply-guard |
| 误认为 Gateway/Controller 会自动配置网络 | 用户预期错位 | UI 常驻零侵入文案 + 文档 + `wifisyncctl plan --dry-run` 恒空证明 |
| 从 Gateway 拉取 Wi-Fi 需凭据 | 安全/侵入顾虑 | 只读拉取、凭据本地保存、用户显式授权、**永不写回** |
| `custom` 来源 + 同设备 ⇒ 改本机 Wi-Fi | 断网风险 | 二次确认 + dry-run 预览 + apply-guard 回滚 + 醒目提示 |
| DSA/swconfig 差异 | 建桥失败 | 双路径探测 + dry-run 输出对比 + 失败即回滚 |
| 802.11r 与中继/客户端兼容性 | 漫游异常 | 自动降级 `ft_over_ds`；UI 显示能力探测结果 |
| `wpad-basic` 缺 KVR | 功能不可用 | 启动前检测并阻止开启 KVR，给出 `opkg install wpad` 指引 |
| 未来多网桥/VLAN 需求导致模型返工 | 重构成本 | 从一开始就用列表结构（`bridges[]`/`vlans[]`），默认单桥单 VLAN |
| **停止时恢复把用户后续配置覆盖** | 用户配置丢失 | 默认 `restore_mode=managed_only` 只还原 wifisync 改过的键；`full` 需二次确认；提供 `--no-restore` 逃生开关 |
| **停止时恢复失败导致失联** | 设备不可达 | 恢复前 sha256 校验；失败重试 1 次 → 回退 `/rom` 配置；基线永不删除；恢复日志留存 |
| **基线建立前就被改动** | 无法还原 | `pre_start` 早于一切探测/写入执行；基线不存在时**拒绝**任何写入（fail-closed） |
| **`kill -9`/断电导致未恢复** | 网络停在半配置态 | 脏标记 + 下次启动自检（提示或自动先恢复）；`pre-change` 快照兜底 |
| 快照占用 flash | 空间不足 | 仅存文本；`initial/` 不裁剪，滚动快照各保留 5 份并设体积上限；超限优先裁剪最旧快照 |

---

## 14. 测试策略

- **单元测试**（host，`cargo test`）：角色矩阵、**建桥策略（穷举 2³ 角色组合，断言仅纯 AP 建桥）**、Wi-Fi 来源解析与禁用、`NetworkProfile` 版本合并、多网桥/VLAN 列表、KVR 渲染、准入状态机、failsafe 状态机、**`managed_keys` 计算与恢复计划、保留策略**。
- **非侵入性测试**：对 `controller`/`gateway`/`controller+gateway` 三种配置，断言 `plan` 输出与 uci diff **恒为空**。
- **生命周期测试**：`start → stop` 各一轮后与初始基线 diff=0；`start → 改配置 → stop` 后恢复到基线；`start → kill -9 → start` 能识别脏状态并恢复；篡改备份文件后恢复被拒绝。
- **dry-run**：`wifisyncctl plan --dry-run` 输出将写入的 uci diff，CI 快照比对。
- **QEMU 实验**：OpenWrt x86/64 + `kmod-mac80211-hwsim` 造虚拟无线 → 拓扑（Gateway / Controller / 纯 AP）+ 中继场景，脚本化 `scripts/qemu-lab.sh`。
- **回归**：准入拒绝不改配置、断链恢复、断电、并发 apply、uci 损坏恢复、**启停循环 20 次无累积漂移**。
- **真机抽测**：至少一款 MIPS 无线路由 + 一款 ARM 无线路由（含无无线设备的 AP 禁用行为）。

---

## 15. 架构与 CI 矩阵（OpenWrt 25.12 全部 x86 / ARM64）

依据：`downloads.openwrt.org/releases/25.12.0/packages/` 的包架构目录
＋ `openwrt-25.12` 分支各 target 的 `target.mk` 里的 `CPU_TYPE`。
映射表的**唯一实现**是 `scripts/openwrt-arch.sh`（CI 与开发者共用同一份）。

| OpenWrt 包架构 | SDK target/subtarget | Rust target | `-C target-cpu` |
|----------------|---------------------|-------------|-----------------|
| `x86_64` | `x86/64` | `x86_64-unknown-linux-musl` | `generic` |
| `i386_pentium4` | `x86/generic` | `i686-unknown-linux-musl` | `pentium4` |
| `i386_pentium-mmx` | `x86/legacy` | `i586-unknown-linux-musl` | `pentium-mmx` |
| `aarch64_generic` | `armsr/armv8` | `aarch64-unknown-linux-musl` | `generic` |
| `aarch64_cortex-a53` | `mediatek/filogic` | `aarch64-unknown-linux-musl` | `cortex-a53` |
| `aarch64_cortex-a72` | `bcm27xx/bcm2711` | `aarch64-unknown-linux-musl` | `cortex-a72` |
| `aarch64_cortex-a76` | `bcm27xx/bcm2712` | `aarch64-unknown-linux-musl` | `cortex-a76` |

包按「包架构」发布，所以用某个 target 的 SDK 编出来的包可装在所有同 `CPU_TYPE`
的 target 上 —— 与 OpenWrt 官方 buildbot 的做法一致。

| 工作流 | 触发 | 内容 |
|--------|------|------|
| `.github/workflows/ci.yml` | push / PR / 手动 | `fmt` + `clippy -D warnings` + `cargo test` + 假根目录端到端冒烟（强制校验「非 AP 零改动 / 纯 AP 建桥」）＋ `shellcheck`/JSON/`node --check` ＋ 7 架构 SDK 工具链交叉编译 + 3 MiB 体积门禁 ＋ `gh-action-sdk` 构建 LuCI 包 |
| `.github/workflows/openwrt-packages.yml` | 手动 / 每月 | 7 架构的官方 `.apk`/`.ipk`（在 SDK 内 `feeds install -a` 后编译，含 rustc 引导，30–90 分钟/架构） |
| `.github/workflows/release.yml` | tag `v*` | 7 架构编译 + sha256 + 发布到 GitHub Release |

开工前请把 `WIFISYNC_OPENWRT_RELEASE`（默认 `25.12.5`）改成实际要跟踪的版本；
SDK 压缩包名由 `scripts/sdk-env.sh` 从官方目录索引自动解析，不需要跟着工具链小版本改。

---

## 16. 实现进度与设计偏差（首轮代码落地）

### 16.1 已完成

| 里程碑 | 状态 | 产出 |
|--------|------|------|
| M0 可行性验证 | ✅ 完成（工具链/架构映射/编译链路） | `scripts/openwrt-arch.sh`、`scripts/sdk-env.sh`、`scripts/build-musl.sh`、CI 三件套 |
| M1 核心逻辑库 | ✅ 完成 | `wifisync-core`：`role` / `capability` / `bridge` / `wifi_source` / `profile` / `admission` / `backup` / `failsafe` / `plan` / `uci_file` / `config`（62 个单测） |
| M2 系统适配层 | ✅ 完成（只读探测 + 快照/恢复 + AP 写入通道） | `wifisync-sys`：`sysfs` / `iwinfo` / `uci` / `netifd` / `snapshot` / `restore` / `state` / `exec` / `paths`（23 个单测） |
| M3 守护进程与安全护栏 | ✅ 主体完成 | `wifisync` 单二进制：`daemon`（生命周期 + 准入 + 下发 + 看门狗）、`rpc`、`ctl`、`ubus`(rpcd 插件)、`secrets`、`probe`、`signals`、`log`（14 个单测） |
| M4 LuCI 应用 | ✅ 首版完成 | `luci-app-wifisync` 8 个页面 + 菜单/ACL（JS 通过 `node --check`） |
| M5 打包与发布 | ✅ 首版完成 | `openwrt/package/wifisync`（Makefile + init.d + 默认 uci + rpcd 桥 + ACL）、三条 workflow |
| M6 联调与验收 | ⏳ 待做 | 需要 QEMU+hwsim 与真机；`docs/BUILDING.md` 已给出演练方式 |

代码规模：**99 个单测全部通过**，`clippy -D warnings` 干净，`cargo fmt --check` 通过。

### 16.2 与初版计划的偏差（及原因）

| 原计划 | 实际实现 | 原因 |
|--------|---------|------|
| tokio 异步运行时 | **std 线程 + 阻塞 IO** | 服务模型足够简单；省掉一个巨大运行时，二进制更小、musl 更稳 |
| rtnetlink 建桥 | **uci + ubus/netifd** | OpenWrt 惯用做法，天然处理 DSA/swconfig 差异 |
| httparse 做本地 API | **一行一个 JSON 的 UNIX socket 协议** | 少一层协议面、少两个依赖 |
| 静态链接（`+crt-static`） | **动态链接设备 musl**（`-crt-static`） | 与官方 `rust-values.mk` 一致，体积更小；静态方案仍保留在 `[profile.release]` 之外按需启用 |
| 检测完整版 wpad 用包管理器查询 | **在 hostapd/wpad 二进制里查找 `mobility_domain` + `ieee80211r`** | opkg（ipk）与 apk 两套数据库格式不同，二进制特征两种系统都适用 |
| 计划里未提 `roles_configured` | 新增该 uci 选项 | 需求 3 要求「无无线时在**默认设置中**取消 AP」：安装时无法知道硬件，因此首次启动按能力算默认值并持久化。**无线存在性以 `/sys/class/ieee80211/*` 为权威**，残留 uci 配置不算（有对应回归测试） |
| 计划里 `wifisyncctl` 独立二进制 | 合并为**单二进制 `wifisync`**（daemon/ctl/ubus 三模式） | 包体积与打包复杂度都更优；rpcd 桥也因此只是一行 `exec` |
| 计划里「失败重试 1 次 → 回退 /rom」 | 已实现回退 `network restart`；`/rom` 兜底与重试留待 M6 真机验证 | 需要真机验证恢复路径的时序（procd 停止顺序） |

### 16.3 下一步（M6）

1. QEMU（x86/64 + `kmod-mac80211-hwsim`）三节点拓扑：Gateway / Controller / 纯 AP，验证
   「非 AP 零改动」在真实 netifd 下的表现；
2. 真机验证 `pre_stop` 恢复的时序（procd 先 SIGTERM → 守护进程同步恢复 → `stop_service` 再补一次幂等恢复）；
3. Controller ↔ AP 的心跳报文协议落地（当前 `secrets.rs` 的 `seal/open` 已就绪，但尚未接线）；
4. 若需要 MIPS 目标，再评估 `ring` 之外的替代（本项目当前只承诺 x86 / ARM64）。
