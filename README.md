# WifiSync

面向 **OpenWrt** 的家庭局域网（含无线）组网工具。Rust 实现，单二进制，带 LuCI 界面。

```text
wifisync daemon     # 常驻服务（procd 拉起）：准入、下发、看门狗、生命周期备份/恢复
wifisync status     # 命令行 / 也通过 ubus 暴露给 LuCI
wifisync ubus ...   # rpcd exec 插件，LuCI 用标准 rpc.declare 调用
```

---

## 1. 设计原则（三条，贯穿全部代码）

| 原则 | 含义 | 代码落点 |
|------|------|---------|
| **零侵入** | Gateway / Controller **不产生任何网络配置写入** | `wifisync-core::plan::build_write_plan` —— 非 AP 角色直接返回空计划 |
| **可回退** | 启动先备份初始化网络，停止先恢复原有网络 | `wifisync-sys::snapshot` / `restore` + `daemon::start/stop` |
| **最小职责** | Controller = 验证 + 下发；Gateway = 标识 + 探测端点；AP = 应用 + 上报 | `wifisync-core::role` |

## 2. 角色模型

| 角色 | 默认 | 职责 | 对本机网络的改动 |
|------|------|------|-----------------|
| `gateway` | 有无线时默认开 | 只要求用户**选定对应的 LAN 接口**（用于识别与探测） | **无**（不写 uci、不动 NAT/防火墙） |
| `controller` | 有无线时默认开 | ① 新 AP 准入验证 ② 网络信息下发（含 Wi-Fi） | **无**（与 Gateway 的连通性由用户单独确认） |
| `ap` | 有无线时默认开；**无无线时强制关闭** | 同步 Controller 下发的网络信息并应用 | 应用下发的 Wi-Fi / 网桥信息（须已准入） |

**建桥规则（唯一条件）**

```text
enable_bridge = roles.ap && !roles.gateway && !roles.controller
```

* 只有「纯 AP」设备才把所有网口（含出厂 WAN 口）并入 `br-lan`；
* 其它任何组合都不建桥、不动网口；
* 数据结构是多网桥 + VLAN 列表（`Vec<BridgePlan>` / `vlans[]`），为将来「多网桥 / VLAN 跨设备网桥同步」预留，默认只产出一个 `br-lan`。

**角色决策矩阵**

| 条件 | 默认角色 | 本机配置写入 | 网桥 |
|------|---------|-------------|------|
| 有无线 + 主路由 | `controller + gateway + ap` | 仅 Wi-Fi | 不建桥 |
| 有无线 + 纯 AP | `ap` | Wi-Fi + 建桥 | 全部网口 → `br-lan` |
| 有无线 + AP 与 Controller 同设备 | `controller + ap` | 仅 Wi-Fi（`custom` 来源时含本机 Wi-Fi） | 不建桥 |
| 无无线 + 主路由 | `controller + gateway`（AP 置灰禁用） | **无** | 不建桥 |

## 3. Wi-Fi 信息源（Controller）

| 来源 | 取值方式 | 禁用条件 |
|------|---------|---------|
| `controller_self` | 读取 Controller 本机无线配置，**只作为模板下发** | Controller 上报无 Wi-Fi |
| `gateway` | **只读**拉取 Gateway 的 Wi-Fi 档案（绝不写回 Gateway） | Gateway 上报无 Wi-Fi / 未选定 / 拉取失败 |
| `custom` | 用户在 LuCI 手工填写 | 始终可用 |

* 只有本机承担 **AP** 角色时才会把解析结果写回本机无线配置；
* 选 `custom` 且 **AP 与 Controller 同设备** ⇒ 会修改该设备自身的 Wi-Fi，必须显式确认
  （`local_wifi_change_confirmed`），并在写入前自动保存快照；
* 禁用逻辑在 LuCI 与 daemon 两侧**双重校验**。

## 4. KVR 与中继

* 802.11k / 802.11v / 802.11r 参数随 `NetworkProfile` 由 Controller 统一下发，
  `mobility_domain` 全局一致（`wifisync-core::profile::KvrConfig`）。
* 需要**完整版 `wpad`**（`wpad-mbedtls` / `wpad`）；检测到 `wpad-basic-mbedtls` 时
  程序**拒绝**开启 KVR 并给出提示（检测方式：在 hostapd/wpad 二进制里查找
  `mobility_domain` + `ieee80211r` 字符串，opkg 与 apk 系统都适用）。
* **不实现 mesh**：Wi-Fi 中继一律走标准中继 = `wpa_supplicant(STA)` 上行 + `hostapd(AP)` 下行。

## 5. 备份与恢复（生命周期）

```text
/etc/wifisync/backup/
├─ initial/            # 不可变初始化基线（权威还原源）
│  ├─ manifest.json    # 每文件 sha256 + managed_keys
│  ├─ config/{network,wireless,dhcp,firewall,system}
│  └─ state.txt        # ip -o link/addr/route + iwinfo + 网桥成员
├─ pre-start-<ts>/     # 每次启动前（保留 5 份）
└─ pre-change-<ts>/    # 每次写入前（保留 5 份，供 apply-guard 回滚）
```

| 时机 | 行为 |
|------|------|
| 服务启动前 | 首次创建不可变基线；已存在则校验完整性；再拍一份 `pre-start` 快照。**没有基线就拒绝启动（fail-closed）** |
| 写入前 | 拍 `pre-change` 快照 + 记录 `managed_keys` |
| 服务停止前 | **同步**恢复原有网络（默认 `restore_mode=managed_only`：只还原 wifisync 改过的键），校验后才退出 |
| 异常退出后 | 脏标记 + 下次启动自检提示 |

* 恢复前重新计算 sha256，校验不过**直接拒绝**且**永不删除基线**；
* 逃生开关：`wifisync stop --no-restore`（或创建 `/etc/wifisync/no-restore-on-stop`）。

## 6. 故障恢复（可选，默认关闭）

| 层级 | 触发 | 动作 |
|------|------|------|
| L1 apply-guard | 应用后 `apply_confirm_secs` 内未确认 | 回滚到写入前快照 |
| L2 link-watchdog | **仅 AP**：与 Controller/Gateway 心跳丢失 > `link_timeout_secs` | 恢复设备默认网络（或恢复后重启） |
| L3 自愈 | 守护进程崩溃 | procd `respawn` + 启动自检 |

Gateway / Controller 不触发网络回滚（零侵入），仅告警。

## 7. 安全模型（为什么没有 TLS）

* 本地 API：UNIX socket `/var/run/wifisync/wifisync.sock`，权限 `0600`，仅 root 可连；
* 节点间：HMAC-SHA256 认证 + ChaCha20-Poly1305 载荷加密（纯 Rust）；
* **刻意不引入 TLS**：`openssl-sys` 在 musl/交叉环境下麻烦，`ring` 不支持 MIPS，
  `aws-lc-rs` 需要 cmake。Web 界面的 HTTPS 交给 OpenWrt 自带的 uhttpd。

## 8. 目录结构

```text
crates/wifisync-core/   纯逻辑（无系统调用，可宿主机单测）
crates/wifisync-sys/    uci / sysfs / iwinfo / netifd / 快照 / 恢复
crates/wifisync/        单二进制：daemon + ctl + rpcd(ubus) 三种模式
openwrt/package/wifisync/   OpenWrt 包（Makefile + init.d + 默认配置 + rpcd 桥 + ACL）
luci/luci-app-wifisync/     LuCI 应用（8 个页面）
scripts/                构建脚本（OpenWrt SDK 工具链交叉编译 + 架构映射）
.github/workflows/      CI（质量门禁 / 多架构构建 / 官方包构建 / 发布）
```

## 9. 快速开始

### 安装（从源码构建的包）

```sh
# 在 OpenWrt 构建树里把本仓库加为 feed
echo "src-link wifisync /path/to/WifiSync" >> feeds.conf.default
./scripts/feeds update wifisync && ./scripts/feeds install -a -p wifisync
make menuconfig      # Network → wifisync / LuCI → Applications → luci-app-wifisync
make package/wifisync/compile V=s
```

### 使用

```sh
# 1. 看能力与默认角色（无无线设备的默认角色里没有 AP）
wifisync status

# 2. 看会做哪些改动（非 AP 角色恒为 0 项）
wifisync plan

# 3. 应用（只有纯 AP 才会真的写）
wifisync apply && wifisync confirm

# 4. 备份 / 恢复
wifisync backup list
wifisync backup verify
wifisync restore              # 默认 managed_only
wifisync restore --full       # 按基线整体覆盖
```

LuCI：**服务 → WifiSync**，共 8 个页面：状态总览 / 角色 / Gateway / Controller / 网桥 / Wi-Fi 与 KVR / 备份与故障恢复 / 诊断。

## 10. 开发

```sh
cargo test                    # 99 个单测：角色矩阵、建桥策略、备份、故障恢复、零侵入不变量
cargo clippy --workspace --all-targets -- -D warnings
scripts/build-musl.sh aarch64_cortex-a53   # 用 OpenWrt SDK 工具链交叉编译
```

构建与 CI 细节见 [`docs/BUILDING.md`](docs/BUILDING.md)，实施计划与需求追溯见 [`PLAN.md`](PLAN.md)。

## 11. 许可

GPL-2.0-only，见 [`LICENSE`](LICENSE)。
