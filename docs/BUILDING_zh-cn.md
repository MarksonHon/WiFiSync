[English](BUILDING.md) | **简体中文**

# 构建与 CI

本项目有三条构建路径，全部**复用 OpenWrt 官方工具链**，并按 OpenWrt 官方
「包架构（package architecture）」而非内核架构来产出。

发行只覆盖两个版本：**OpenWrt 24.10**（`.ipk` / opkg）与 **OpenWrt 25.12**
（`.apk` / apk）；更老的版本既不在测试范围也不发行。

---

## 1. 支持的架构（OpenWrt 24.10 / 25.12 的全部 x86 / ARM64）

依据：`downloads.openwrt.org/releases/<版本>/packages/` 的架构目录
＋ `openwrt-24.10` / `openwrt-25.12` 分支各 target 的 `target.mk` 中的 `CPU_TYPE`。
两个版本的包架构与 SDK target/subtarget 名称完全一致，即下表这七种。

| OpenWrt 包架构 | SDK target/subtarget | Rust target | `-C target-cpu` |
|----------------|---------------------|-------------|-----------------|
| `x86_64` | `x86/64` | `x86_64-unknown-linux-musl` | `generic` |
| `i386_pentium4` | `x86/generic` | `i686-unknown-linux-musl` | `pentium4` |
| `i386_pentium-mmx` | `x86/legacy` | `i586-unknown-linux-musl` | `pentium-mmx` |
| `aarch64_generic` | `armsr/armv8` | `aarch64-unknown-linux-musl` | `generic` |
| `aarch64_cortex-a53` | `mediatek/filogic` | `aarch64-unknown-linux-musl` | `cortex-a53` |
| `aarch64_cortex-a72` | `bcm27xx/bcm2711` | `aarch64-unknown-linux-musl` | `cortex-a72` |
| `aarch64_cortex-a76` | `bcm27xx/bcm2712` | `aarch64-unknown-linux-musl` | `cortex-a76` |

映射表的**唯一实现**是 `scripts/openwrt-arch.sh`：

```sh
scripts/openwrt-arch.sh list
scripts/openwrt-arch.sh aarch64_cortex-a53 all
# → mediatek/filogic aarch64-unknown-linux-musl cortex-a53
```

包是按**包架构**发布的，因此用某个 target 的 SDK 编出来的包可以装在
所有使用同一 `CPU_TYPE` 的 target 上（与 OpenWrt 官方 buildbot 的做法一致）。

---

## 2. 路径 A：用 SDK 工具链直接交叉编译（快，本地用）

```sh
# 1) 下载官方 SDK 并导出工具链（自动解析 SDK 文件名）
. scripts/sdk-env.sh x86/64

# 2) 一条命令完成：架构映射 → 工具链 → 编译 → 产物与信息
scripts/build-musl.sh x86_64
# 产物: dist/x86_64/wifisync  +  build-info.json
```

`scripts/build-musl.sh` 与 OpenWrt 官方 Rust 构建（`feeds/packages/lang/rust/rust-values.mk`）
保持一致的几个关键点：

| 项 | 取值 | 原因 |
|----|------|------|
| 链接器 | SDK 的 `*-musl-gcc` | 与官方包同源 |
| `-C target-feature=-crt-static` | 动态链接设备自带 musl | 官方 rust-values.mk 就是这么设的，体积更小 |
| `-C target-cpu=<cpu>` | 按上表 | 对齐 OpenWrt 的 `CPU_TYPE` |
| `--locked` | 使用提交进仓库的 `Cargo.lock` | 可复现 |
| `CARGO_PROFILE_RELEASE_*` | `lto=true / opt-level=z / codegen-units=1 / debug=false` | 与 rust-values.mk 相同 |

体积门禁：**3 MiB**（小 flash 设备）。由 `release.yml` 从打好的包里取回二进制做检查，
超限即失败——SDK 默认开着 `CONFIG_AUTOREMOVE`，构建一结束就会删掉 `build_dir`。

## 3. 路径 B：官方包格式（.apk / .ipk，慢）

这条路径完全照 OpenWrt 官方 Rust 包的流程：在 SDK 里
`feeds install -a` 拿到 `feeds/packages/lang/rust`，再由 `rust-package.mk` 的
`Build/Compile/Cargo` 调 `cargo install`，最后走官方打包逻辑。

```sh
TARGET=x86/64
. scripts/sdk-env.sh "$TARGET"
cd "$WIFISYNC_SDK"

# 本仓库即 feed
echo "src-link wifisync $OLDPWD" >> feeds.conf.default
cp feeds.conf.default feeds.conf
./scripts/feeds update -a
./scripts/feeds install -a
make defconfig

# 用宿主机的 Rust 工具链（见下）
export WIFISYNC_HOST_RUST=1
rustup target add x86_64-unknown-linux-musl     # 上面 target 对应的 triple

# 目录目标只构建这一个包。NO_DEPS 去掉它的运行时依赖（wpad、kmod-br-netfilter）：
# 那些由设备上的包管理器解决，在这里构建会连带拉入 hostapd 与整个内核。
make package/feeds/wifisync/wifisync/compile NO_DEPS=1 V=s -j$(nproc)

# LuCI 应用与翻译包（与架构无关）。先构建 luci-base：编译翻译所需的 po2lmo 宿主工具由它提供；
# 再用 NO_DEPS 阻止应用包构建去解析 wifisync（否则会连带拉入 hostapd 与内核）。
make package/feeds/luci/luci-base/compile V=s -j$(nproc)
make package/feeds/wifisync/luci-app-wifisync/compile NO_DEPS=1 V=s -j$(nproc)

find bin/packages -name '*.apk' -o -name '*.ipk'
```

> OpenWrt 25.12 起默认包管理器是 **apk**，之前是 opkg；产物格式随分支自动变化：
> 25.12 出 `.apk`，24.10 出 `.ipk`。
> `WIFISYNC_HOST_RUST=1` 让 Rust 部分用宿主机工具链编译（上面的 `rustup target add` 提供目标 std），
> SDK 只负责推导 target 与打包；否则 SDK 会在内部从源码引导 rustc（单架构约 30–90 分钟）。
> `release.yml` 构建的正是这些包，用手动触发并填上要发布的 tag（见第 5 节）。

如果要在自己的 OpenWrt 构建树里编译：

```sh
echo "src-link wifisync /path/to/WifiSync" >> feeds.conf.default
./scripts/feeds update wifisync && ./scripts/feeds install -a -p wifisync
make defconfig
make package/wifisync/compile V=s
```

## 4. 路径 C：本机开发

```sh
cargo build            # 宿主机构建（调试）
cargo test             # 101 个单测
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# 用假 OpenWrt 根目录做端到端演练（不需要路由器）
export WIFISYNC_ROOT=$(mktemp -d)
cargo run -- plan      # 角色为 controller+gateway 时输出 "0 项改动"
```

`WIFISYNC_ROOT` 会把所有路径（`/etc/config`、`/sys/class/net`、`/etc/wifisync`…）
重定向到该目录，方便在开发机上验证探测、规划、快照与恢复逻辑。

---

## 5. CI 组成

| 工作流 | 触发 | 内容 |
|--------|------|------|
| `ci.yml` | push / PR / 手动 | ① `fmt` + `clippy -D warnings` + `cargo test` + 端到端冒烟测试（均按 `x86_64-unknown-linux-musl` 目标编译）<br>② `shellcheck` + JSON 校验 + `node --check`（LuCI JS）+ 视图 `_()` 词条与 `po/*` 的覆盖比对 + 后端消息键与前端 `MESSAGES` 表的一致性比对<br>③ 7 个架构用 SDK 工具链交叉编译 + 体积门禁 + 上传产物<br>④ 用 `openwrt/gh-action-sdk` 构建 `luci-app-wifisync` 验证 feed 包结构 |
| `openwrt-packages.yml` | 手动 / 每月 | 7 个架构的**官方 .apk/.ipk** 构建（慢，含 rustc 引导） |
| `release.yml` | 打 tag `v*` | 7 个架构编译 + sha256 + 发布到 GitHub Release（含架构对照表） |

### 冒烟测试里被强制的「零侵入」不变量

`ci.yml` 的 `lint-test` 任务会在假根目录里跑两条断言（需求 5 / 7 / 8 的守门人）：

```sh
# 角色 = controller gateway  ⇒ 写入计划必须为空
wifisync plan | grep -q '"empty": true'

# 角色 = ap ⇒ 必须出现 br-lan，且包含 802.11r
wifisync plan | grep -q '"name": "br-lan"'
wifisync plan | grep -q 'ieee80211r'
```

对应的单元测试更彻底：`plan.rs::gateway_and_controller_are_non_invasive`
穷举 2³ 全部角色组合，断言非 AP 组合的计划恒为空。

---

## 6. 依赖约束（musl 可编译性）

| 用途 | 选用 | 说明 |
|------|------|------|
| 序列化 | `serde` + `serde_json` | 纯 Rust |
| 哈希 | `sha2` | 备份清单校验 |
| 认证/加密 | `hmac` + `chacha20poly1305` | 节点间心跳；纯 Rust，全架构可用 |
| 信号 | `libc` | `sigaction` 装 SIGTERM/SIGINT/SIGHUP |
| 错误 | `thiserror` | 编译期宏，无运行时开销 |
| **不使用** | `tokio`、`rtnetlink`、`hyper`/`httparse`、`openssl-sys`、`ring` | 见下 |

刻意**没有**使用的东西，以及原因：

* **tokio**：服务只需要「UNIX socket + 线程 + 定时器」，std 足够，省掉一个巨大的运行时；
* **rtnetlink**：网桥落地交给 netifd（写 uci `config device type bridge` + `ubus call network reload`），
  这才是 OpenWrt 的惯用做法，也避免自己处理 DSA 差异；
* **httparse/hyper**：本地 API 用一行一个 JSON 的 UNIX socket 协议；
* **openssl-sys / ring / aws-lc-rs**：前者在 musl 交叉下麻烦，`ring` **不支持 MIPS**，
  后者需要 cmake；因此本地在 socket 上做权限隔离，节点间用 HMAC + ChaCha20-Poly1305。

---

## 7. 常见问题

**Q: 为什么 `i386_pentium-mmx` 用 `x86/legacy`？**
A: `x86/generic` 的 `CPU_TYPE := pentium4`（对应 `i386_pentium4`，Rust 用 `i686`）；
`x86/legacy` 没有指定 `CPU_TYPE`，走 i486/pentium-mmx 档，Rust 侧对应 `i586-unknown-linux-musl`。

**Q: 二进制还是偏大？**
A: 按需裁剪：`[profile.release]` 已开 LTO/`opt-level=z`/`panic=abort`/`strip`；
若仍超门禁，可考虑用 nightly 的 `-Z build-std` + `panic_immediate_abort`（CI 里尚未启用）。

**Q: 怎么发布一个版本？**
A: 在 Actions 页面手动运行 `release` 工作流，并填写要发布的 tag（例如 `v0.1.0`）。
tag 不存在时会在本次 dispatch 的 commit 上创建；若该 tag 已有 release，则删除后重建。
包属于发布产物（14 个架构任务会产生数百 MB），因此 `ci.yml` 只做源码检查，打包由 `release.yml` 负责。

**Q: 发行覆盖哪些 OpenWrt 版本？**
A: 只有 24.10（`.ipk` / opkg）与 25.12（`.apk` / apk），各七个包架构（见第 1 节）。
两个分支共用同一份 `scripts/openwrt-arch.sh` 映射，SDK target/subtarget 名称也完全一致。

**Q: 中继模式会在哪里体现？**
A: 见 [`FRONTEND_zh-cn.md`](FRONTEND_zh-cn.md) 的「Wi-Fi 与 KVR」页说明与
[`BACKEND_zh-cn.md`](BACKEND_zh-cn.md) 的 M6；
实现为 `wpa_supplicant(STA)` + `hostapd(AP)`，**没有任何 mesh（802.11s）代码路径**。
