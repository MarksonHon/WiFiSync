**English** | [简体中文](STEERING_zh-cn.md)

# 客户端引导（Steering）设计

> 状态：**提案 — 尚未实现。**
> 范围：内建于 `wifisync` 的自包含客户端引导子系统（不引入外部守护进程）。
> 硬前提：完整版 `wpad` 包（提供 hostapd ubus 接口）——已是本包的依赖。
> 默认：**关闭**；首次启用仅允许演练模式。

相关文档：[`BACKEND_zh-cn.md`](BACKEND_zh-cn.md)（角色、KVR 下发、生命周期）·
[`FRONTEND_zh-cn.md`](FRONTEND_zh-cn.md)（ubus 契约）· [`BUILDING_zh-cn.md`](BUILDING_zh-cn.md)（构建与 CI）。

---

## 1. 定位

这里的引导指：**服务运行期间**观察客户端站点，当某个客户端明显更适合另一个 BSS 时，请求它迁移
（802.11v BSS Transition Management），或在明确开启时才强制迁移。

### 1.1 与既有设计的关系

`wifisync` 目前只下发 802.11k/v/r 参数并分发网络信息，从不干预正在连接的客户端。引导是在其之上
新增的**运行期动作层**。

既有流程不变：配置面（角色、准入、网桥、KVR、备份/恢复）语义保持原样。引导是一个额外的、可
独立开关的子系统。

### 1.2 非目标

* 不做 mesh（802.11s）——维持不变。
* 不做射频规划、信道选择、频谱管理。
* 客户端不装 agent，只用标准 802.11k/v 信令。
* 不写网络配置：除既有 KVR/网桥流程外，引导永不触碰 uci `network`/`wireless`。
* 不引入 `usteer`/`DAWN`：项目约束是"不引入外部代码或外置组件"，而二者都是自带配置与 init
  脚本的独立守护进程。

### 1.3 范式偏离（有意留痕）

`BACKEND_zh-cn.md` 声明了三条横切原则：**零侵入**、**可逆**、**最小责任**。引导偏离了其中两条，
故在此留痕：

| 原则 | 之前 | 引入引导后 |
|------|------|-----------|
| 零侵入 | 运行期完全不干预 | **网络配置仍不被触碰**，但客户端可能被请求迁移。默认关闭。 |
| 可逆 | 任何改动都能从基线恢复 | 被引导的客户端可能连不回来。靠演练模式、白名单、频率上限、急停开关补偿——并非真正可逆。 |
| 最小责任 | Controller 校验/分发，AP 应用 | 形态不变：**Controller 决策，AP 执行**。 |

推论：引导**默认关闭**，首个模式是演练模式，且第 8 节全部生效。

### 1.4 来自项目规则的约束

* 不新增第三方 crate 或二进制。只用 `ubus`（诊断已在用）与完整版 `wpad` 的 hostapd 接口
  （已是依赖）。
* 新增的 CLI 输出与日志一律纯英文；只有 LuCI 语言包做本地化。
* LuCI 不含任何引导逻辑：只调用 `common.js` 中声明的 ubus 方法。

---

## 2. 架构 —— Controller 决策，AP 执行

`usteer` 与 `DAWN` 是点对点的：每个 AP 各自维护站点视图，与对等节点交换（UDP 广播 / TCP +
uMDNS），并在本地决策。它们没有中心节点，所以必须自己解决节点发现与状态一致性。

`wifisync` **已经具备**中心节点（Controller）、认证通道、准入，以及版本化分发机制（R8/R9）。
因此引导把拓扑反转过来：

```
             ┌────────────── Controller（决策方）────────────────────┐
             │  站点视图聚合 → 策略评估 → 决策                        │
             │  （每次评估都记录理由与证据）                          │
             └───────▲───────────────────────────────────────┬───────┘
                     │ StationReport（HMAC + ChaCha20-Poly1305）│ SteerCommand（版本化）
          ┌──────────┴──────────┐                  ┌──────────┴──────────┐
          │ AP（执行方）         │                  │ AP（执行方）         │
          │ hostapd ubus 适配层  │                  │ hostapd ubus 适配层  │
          └─────────────────────┘                  └─────────────────────┘
```

设计推论：

* **无需节点发现、无需共识。** AP 之间从不通信。
* **单决策点 ⇒ 可解释。** 所有决策连同证据集中在一条日志里——这是相对复用外部守护进程的主要优势。
* **Controller 不是安全关键的单点。** 若它不可达，AP 直接停止引导（§8.5）。安全默认是"不干预"，
  所以 Controller 掉线退化为当前行为，而不是更危险的行为。

---

## 3. 数据模型（`wifisync-core::steering`）

```rust
/// 某 AP 对某 BSS 上某站点的一次观测。
pub struct StationObservation {
    pub observed_at_ms: u64,   // 本地单调时钟
    pub ap_id: String,         // device_id（与准入用的同一身份）
    pub bssid: String,
    pub ssid: String,
    pub freq: u32,
    pub signal_dbm: i32,
    pub connected: bool,       // 当前是否关联到该 BSS
    pub num_sta: u32,          // 该 radio 已连站点数（负载输入）
    pub channel_load: Option<u8>,
}

/// Controller 侧聚合：客户端 MAC → 窗口内全部观测。
pub struct StationView { /* BTreeMap<Mac, Vec<StationObservation>> */ }

pub struct SteeringPolicy {
    pub enabled: bool,
    pub dry_run: bool,                 // 默认 true
    pub min_snr_dbm: i32,              // 低于此值才考虑
    pub min_gain_db: i32,              // 目标至少好这么多
    pub hysteresis: u8,                // 连续 N 次评估一致
    pub cooldown_ms: u64,              // 单客户端最小间隔
    pub max_steers_per_hour: u32,      // 单客户端频率上限
    pub aggressiveness: Aggressiveness, // Btm（默认）| BtmDisassoc | Kick（需显式开启）
    pub ssid_allowlist: Vec<String>,
    pub mac_denylist: Vec<String>,
}

pub enum Decision {
    Hold { reason: Reason },
    Steer { target_bssid: String, target_ap: String, reason: Reason, evidence: Evidence },
}
```

`Decision` 始终携带机器可读的 `reason` 与证据（双方信号、负载、策略版本）。这是 §9.S7 可测的前提。

---

## 4. 节点间协议（复用既有通道）

在现有 HMAC-SHA256 + ChaCha20-Poly1305 通道上新增两类消息——不新增传输、不新增凭据：

| 消息 | 方向 | 内容 | 说明 |
|------|------|------|------|
| `StationReport` | AP → Controller | `version`、`ap_id`、单调时间戳、批量 `StationObservation` | 默认 5s；站点处于连接态或信号变化超过 3 dB 时才纳入 |
| `SteerCommand` | Controller → AP | `policy_version`、`mac`、`target_bssid`、`aggressiveness`、`validity_ms`、`decision_id` | AP 拒绝过期、版本陈旧或不在白名单内的指令 |

规则：

* 时间戳单调，接收时做窗口校验（防重放），与现有心跳 HMAC 的处理一致。
* `policy_version` 单调递增，思路同 `NetworkProfile.version`；AP 忽略版本更旧的指令。
* AP **执行前必须校验**：目标 BSSID 当前确实存在于本 AP、该客户端确实处于关联态、且未超出频率上限。

---

## 5. AP 侧 hostapd 适配层（`wifisync-sys`）

全部经由 `ubus` CLI，与现有 `exec::run("uci" | "iwinfo" | "ip", …)` 风格一致。**不**重实现 ubus
二进制协议（blob 编码）。

| 用途 | 命令 |
|------|------|
| 运行期打开 802.11k/v 能力 | `ubus call hostapd.<iface> bss_mgmt_enable '{ "neighbor_report": true, "beacon_report": true, "link_measurement": true, "bss_transition": true }'` |
| 读取已连站点 | `ubus call hostapd.<iface> get_clients` |
| 读取单客户端详情 | `ubus call hostapd.<iface> get_client_info '{ "addr": "<mac>" }'` |
| 自身邻居报告 | `ubus call hostapd.<iface> rrm_nr_get_own` |
| 发布邻居报告列表 | `ubus call hostapd.<iface> rrm_nr_set '{ … }'` |
| 请求客户端迁移（默认） | `ubus call hostapd.<iface> bss_transition_request '{ … }'` |
| 强制迁移（仅显式开启） | `ubus call hostapd.<iface> wnm_disassoc_imminent '{ … }'` |
| 订阅 hostapd 通知 | `ubus subscribe hostapd.<iface>`（子进程，逐行 JSON） |

适配层要求：

* **字段名陷阱**：hostapd README 写的是 `link_measurements`，而 hostapd 源码用的是
  `link_measurement`（单数）。以单数为准；复数形式静默无效。
* hostapd 重启后必须重新执行 `bss_mgmt_enable`，并重新挂载 `ubus subscribe`（订阅前先
  `ubus wait_for hostapd.<iface>`）。
* 子进程按外部命令统一监管；订阅进程异常退出应退避重启，而不是拖垮守护进程。

### 5.1 配置侧前提（当前是坏的）

802.11v **并没有**被现在使用的 uci 选项名打开：

| uci 选项 | 实际效果（OpenWrt `wifi-scripts` 生成 hostapd 配置时） |
|----------|------------------------------------------------------|
| `ieee80211k 1` | 产出 `rrm_neighbor_report=1` 与 `rrm_beacon_report=1` |
| **`bss_transition 1`** | 产出 hostapd `bss_transition=1` —— 这才是 802.11v 的开关 |
| `ieee80211v 1` | 在 `hostapd.sh` / `ap.uc` 中未找到处理分支；在证伪之前按 no-op 处理 |
| `ieee80211r 1` | 连同 `mobility_domain`、`ft_over_ds`、`ft_psk_generate_local` —— 已正确产出 |

因此当前的写计划（`ieee80211k` / `ieee80211v` / `ieee80211r`）**不会**打开 BTM，而引导依赖它。
修复它既是引导的 S0 里程碑，也是 802.11v 本身的正确性问题。

---

## 6. Controller 侧策略

评估循环（每站点、每策略周期，默认 5s）：

1. 用新鲜窗口内的上报构建站点视图；丢弃过期 AP。
2. 忽略以下站点：SSID 不在白名单、MAC 在黑名单、处于冷却期、或超出 `max_steers_per_hour`。
3. 候选筛选：当前信号低于 `min_snr_dbm`，且同一 SSID 在另一 AP 上有优于 `min_gain_db` 的目标 BSS。
4. 需连续 `hysteresis` 次决策一致才执行（防抖）。
5. 产出 `Hold` 或 `Steer` —— 不论是否 `dry_run`，始终带理由与证据。

**有意不做**：DAWN 式的多因子加权评分。我们可靠拿到的输入——信号、频段、站点数，以及可选的
信道负载——对家庭 ESS 已足够；带显式迟滞的阈值策略远比打分模型容易推理与排查。

---

## 7. 配置（uci）

在 `/etc/config/wifisync` 新增段，**默认关闭**：

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

`aggressiveness` 取值：`btm`（默认，礼貌请求）、`btm_disassoc`（请求并声明即将断连）、
`kick`（强制断连；仅显式开启）。

---

## 8. 安全护栏

1. **默认关闭**；启用需在 LuCI 中显式确认。
2. **默认演练模式** —— 记录决策但不向客户端发送任何东西。
3. **急停开关**：一条 RPC 立即停止全部引导，无需重启服务。
4. **单客户端频率上限**（`max_steers_per_hour`）与尝试间冷却。
5. **失联时 AP 自主**：Controller 不可达超过 `controller_timeout_ms`，AP 即停止执行。安全默认，无需协调。
6. **SSID 白名单 / MAC 黑名单**：Controller 侧强制，AP 侧复查。
7. **引导不参与 failsafe**：不得触发或阻塞 apply-guard 回滚与网络恢复。
8. **中继拓扑降级**：中继链路（802.11r 本就受限）默认禁用引导，并在界面提示。

---

## 9. 需求追溯

| # | 需求 | 设计要点 | 模块 | 验收 |
|---|------|----------|------|------|
| S1 | 802.11v 必须真正开启 | 下发 `bss_transition`，运行期 `bss_mgmt_enable` | `core::plan`、`sys::hostapd` | 生成的 hostapd 配置含 `bss_transition=1`；BTM 请求能到达测试客户端 |
| S2 | AP 向 Controller 上报站点观测 | 既有认证通道上的 `StationReport` | `sys::hostapd`、`daemon` | 双 AP hwsim 下两个 AP 的观测都能到达 Controller |
| S3 | Controller 构建全局站点视图 | 窗口聚合、过期 AP 淘汰 | `core::steering` | 单测 + 界面实时视图 |
| S4 | 策略产出可解释决策 | 阈值 + 迟滞 + 冷却 + 黑白名单 | `core::steering` | 相同评估序列产出相同决策（确定性测试） |
| S5 | AP 执行引导指令 | 默认 BTM；`wnm_disassoc_imminent` 需开启 | `sys::hostapd` | hwsim 切换成功；忽略 BTM 的客户端不被继续打扰 |
| S6 | 演练模式与急停 | 只记录模式；立即停止 RPC | `daemon`、`rpc` | `dry_run=1` 时网络 diff 恒为 0，客户端零影响 |
| S7 | 每次决策可解释 | 记录 `reason` + 证据 + 策略版本 | `core::steering`、`rpc` | 每条日志可回答"为何是这个客户端、为何是这个目标" |
| S8 | Controller 失联的 fail-safe | 超时后 AP 自停 | `daemon` | 拔掉 Controller 后各 AP 停止引导 |
| S9 | 不新增外部依赖 | 仅 ubus CLI + 完整版 wpad | `Cargo.toml`、`Makefile` | 依赖清单与体积门禁不变 |
| S10 | LuCI 页面 | 只用 ubus，不含逻辑；语言包 | `luci-app-wifisync` | ACL/方法走查与 `common.js` 一致；`zh_Hans` 覆盖新增串 |

---

## 10. 里程碑

| # | 内容 | 验收 |
|---|------|------|
| **S0** | 地基验证：802.11v 是否真开；`ubus subscribe hostapd.*` 能否送来所需事件；`get_clients` 返回什么；hwsim 下 BTM 能否真正移动站点 | 验证报告 + 至少一条真实 BTM 切换证据 |
| **S1** | `wifisync-sys` hostapd 适配层（读、订阅、写） | 单测 + 针对**真实** hostapd 的集成测试（非 stub） |
| **S2** | `StationReport` 上报与 Controller 聚合（不含决策） | 两个 hwsim AP 聚合为单一视图 |
| **S3** | 策略 + 决策 + 演练模式（只记录） | 日志能解释每次决策；零实际干预 |
| **S4** | 执行路径（BTM，最低档）+ 安全护栏 | hwsim 切换成功；急停与失联自停均验证 |
| **S5** | LuCI 页面、文档、范式偏离留痕 | 页面可演练、可关闭、可查看理由 |

---

## 11. 测试

* **单测**：策略函数 —— 迟滞、冷却、防抖、黑白名单、频率上限。
* **集成（真实链路）**：QEMU + hwsim 三节点（网关/Controller/AP）双 AP 场景。hostapd 适配层必须
  对**真实** hostapd 运行；返回固定 JSON 的 stub 恰好会掩盖这个特性最需要的那些失败。
* **回归**：`steering.enabled=0` 时可观测行为与现状逐字节一致；`dry_run=1` 时客户端零影响且网络
  diff 为 0。
* **反例**：Controller 掉线 ⇒ AP 停止；不支持 BTM 的客户端 ⇒ 不被骚扰。

---

## 12. 风险

| 风险 | 应对 |
|------|------|
| 强制迁移可能导致客户端连不回来 | 默认 BTM 软请求；`kick` 需显式开启 + 白名单 + 频率上限 |
| 阈值无法从既有项目继承，必须在真实拓扑上标定 | S3 演练模式先收集数据，再执行任何动作 |
| 二进制体积可能超出 3 MiB CI 门禁 | S1 后立即测量；必要时把引导做成同包内的独立二进制 |
| 中继链路本就限制 802.11r，引导同样受限 | 自动降级并在界面提示（§8.8） |
| 802.11v 被静默关闭（见 §5.1） | S0 先修配置链路 |
| `ubus` CLI 每次调用的开销 | 无需批量读；每周期每 radio 一次 `get_clients`，而引导调用本身极低频 |

---

## 13. 动工前的待确认项

1. 在目标 OpenWrt 构建上确认 `ieee80211v` 是否 no-op，以及 `plan.rs` 是否必须补 `bss_transition`（§5.1）。
2. 确认 `ubus subscribe hostapd.<iface>` 实际送来的通知类型及其输出格式。
3. 确认目标 wpad 构建上 `get_clients` / `get_client_info` 的确切输出结构。
4. 确认该 hostapd 构建接受的 `bss_transition_request` 参数集合。
