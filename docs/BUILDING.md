**English** | [简体中文](BUILDING_zh-cn.md)

# Building and CI

This project has three build paths. All of them **reuse the official OpenWrt
toolchain** and produce artifacts keyed by OpenWrt's "package architecture"
rather than the kernel architecture.

---

## 1. Supported architectures (all x86 / ARM64 from OpenWrt 25.12)

Basis: the architecture directories under
`downloads.openwrt.org/releases/25.12.0/packages/`, plus the `CPU_TYPE` values
in each target's `target.mk` on the `openwrt-25.12` branch.

| OpenWrt package arch | SDK target/subtarget | Rust target | `-C target-cpu` |
|----------------------|----------------------|-------------|-----------------|
| `x86_64` | `x86/64` | `x86_64-unknown-linux-musl` | `generic` |
| `i386_pentium4` | `x86/generic` | `i686-unknown-linux-musl` | `pentium4` |
| `i386_pentium-mmx` | `x86/legacy` | `i586-unknown-linux-musl` | `pentium-mmx` |
| `aarch64_generic` | `armsr/armv8` | `aarch64-unknown-linux-musl` | `generic` |
| `aarch64_cortex-a53` | `mediatek/filogic` | `aarch64-unknown-linux-musl` | `cortex-a53` |
| `aarch64_cortex-a72` | `bcm27xx/bcm2711` | `aarch64-unknown-linux-musl` | `cortex-a72` |
| `aarch64_cortex-a76` | `bcm27xx/bcm2712` | `aarch64-unknown-linux-musl` | `cortex-a76` |

The **single implementation** of this mapping table is `scripts/openwrt-arch.sh`:

```sh
scripts/openwrt-arch.sh list
scripts/openwrt-arch.sh aarch64_cortex-a53 all
# → mediatek/filogic aarch64-unknown-linux-musl cortex-a53
```

Packages are published per **package architecture**, so a package built with one
target's SDK can be installed on every target that uses the same `CPU_TYPE`
(this matches what the official OpenWrt buildbots do).

---

## 2. Path A: cross-compile directly with the SDK toolchain (fast, runs on every commit in CI)

```sh
# 1) Download the official SDK and export the toolchain (resolves the SDK filename automatically)
. scripts/sdk-env.sh x86/64

# 2) A single command does it all: arch mapping → toolchain → build → artifacts and info
scripts/build-musl.sh x86_64
# artifacts: dist/x86_64/wifisync  +  build-info.json
```

Key points where `scripts/build-musl.sh` stays consistent with the official
OpenWrt Rust build (`feeds/packages/lang/rust/rust-values.mk`):

| Item | Value | Why |
|------|-------|-----|
| Linker | the SDK's `*-musl-gcc` | same origin as the official package |
| `-C target-feature=-crt-static` | link dynamically against the device's own musl | this is exactly what rust-values.mk does; smaller binaries |
| `-C target-cpu=<cpu>` | per the table above | aligns with OpenWrt's `CPU_TYPE` |
| `--locked` | use the `Cargo.lock` committed to the repository | reproducibility |
| `CARGO_PROFILE_RELEASE_*` | `lto=true / opt-level=z / codegen-units=1 / debug=false` | same as rust-values.mk |

Size gate: **3 MiB** (for small-flash devices). CI fails when the limit is exceeded.

## 3. Path B: official package format (.apk / .ipk, slow)

This path follows the official OpenWrt Rust package flow exactly: inside the SDK,
`feeds install -a` fetches `feeds/packages/lang/rust`, then
`Build/Compile/Cargo` from `rust-package.mk` invokes `cargo install`, and finally
the official packaging logic runs.

```sh
TARGET=x86/64
. scripts/sdk-env.sh "$TARGET"
cd "$WIFISYNC_SDK"

# This repository is the feed
echo "src-link wifisync $OLDPWD" >> feeds.conf.default
cp feeds.conf.default feeds.conf
./scripts/feeds update -a
./scripts/feeds install -a
make defconfig

make package/wifisync/compile V=s -j$(nproc)
find bin/packages -name '*.apk' -o -name '*.ipk'
```

> Starting with OpenWrt 25.12 the default package manager is **apk** (it was
> opkg before), and the artifact format follows the branch automatically.
> This path builds `rust/host` (the rustc bootstrap) from source inside the SDK
> and takes roughly 30–90 minutes per architecture, which is why
> `openwrt-packages.yml` only runs on **manual dispatch / a monthly schedule**.

To build inside your own OpenWrt build tree:

```sh
echo "src-link wifisync /path/to/WifiSync" >> feeds.conf.default
./scripts/feeds update wifisync && ./scripts/feeds install -a -p wifisync
make defconfig
make package/wifisync/compile V=s
```

## 4. Path C: local development

```sh
cargo build            # host build (debug)
cargo test             # 101 unit tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# End-to-end dry run against a fake OpenWrt root (no router required)
export WIFISYNC_ROOT=$(mktemp -d)
cargo run -- plan      # prints "0 changes" when the role is controller+gateway
```

`WIFISYNC_ROOT` redirects all paths (`/etc/config`, `/sys/class/net`,
`/etc/wifisync`…) into that directory, which makes it easy to validate the
probe, plan, snapshot, and restore logic on a development machine.

---

## 5. CI composition

| Workflow | Trigger | Contents |
|----------|---------|----------|
| `ci.yml` | push / PR / manual | ① `fmt` + `clippy -D warnings` + `cargo test` + end-to-end smoke test, compiled for `x86_64-unknown-linux-musl`<br>② `shellcheck` + JSON validation + `node --check` (LuCI JS) + translation catalog coverage + backend/front-end message key consistency<br>③ cross-compile 7 architectures with the SDK toolchain + size gate + upload artifacts<br>④ build `luci-app-wifisync` with `openwrt/gh-action-sdk` to validate the feed package layout |
| `openwrt-packages.yml` | manual / monthly | Official **.apk/.ipk** builds for 7 architectures (slow, includes the rustc bootstrap) |
| `release.yml` | tag `v*` | Build 7 architectures + sha256 + publish to GitHub Release (with an architecture reference table) |

### The "zero intrusion" invariants enforced by the smoke test

The `lint-test` job in `ci.yml` runs two assertions inside the fake root
(the gatekeepers for requirements 5 / 7 / 8):

```sh
# role = controller gateway  ⇒ the write plan must be empty
wifisync plan | grep -q '"empty": true'

# role = ap ⇒ br-lan must be present, and it must include 802.11r
wifisync plan | grep -q '"name": "br-lan"'
wifisync plan | grep -q 'ieee80211r'
```

The corresponding unit test is more thorough:
`plan.rs::gateway_and_controller_are_non_invasive` enumerates all 2³ role
combinations and asserts that the plan is always empty for any non-AP
combination.

---

## 6. Dependency constraints (musl buildability)

| Purpose | Choice | Notes |
|---------|--------|-------|
| Serialization | `serde` + `serde_json` | pure Rust |
| Hashing | `sha2` | backup manifest verification |
| Authentication/encryption | `hmac` + `chacha20poly1305` | inter-node heartbeat; pure Rust, available on all architectures |
| Signals | `libc` | `sigaction` to install SIGTERM/SIGINT/SIGHUP |
| Errors | `thiserror` | compile-time macro, no runtime overhead |
| **Not used** | `tokio`, `rtnetlink`, `hyper`/`httparse`, `openssl-sys`, `ring` | see below |

Things deliberately **left out**, and why:

* **tokio**: the service only needs "UNIX socket + threads + timers"; std is
  enough, and this avoids a huge runtime;
* **rtnetlink**: bridge creation is delegated to netifd (write the uci
  `config device type bridge` section + `ubus call network reload`), which is
  the idiomatic OpenWrt approach and avoids handling DSA differences ourselves;
* **httparse/hyper**: the local API uses a UNIX socket protocol with one JSON
  object per line;
* **openssl-sys / ring / aws-lc-rs**: the first is painful to cross-compile for
  musl, `ring` **does not support MIPS**, and the last one requires cmake.
  Instead, privileges are isolated on the local socket and nodes communicate
  using HMAC + ChaCha20-Poly1305.

---

## 7. FAQ

**Q: Why does `i386_pentium-mmx` use `x86/legacy`?**
A: `x86/generic` sets `CPU_TYPE := pentium4` (that is `i386_pentium4`, which maps
to `i686` on the Rust side); `x86/legacy` does not set `CPU_TYPE` and falls into
the i486/pentium-mmx tier, which corresponds to `i586-unknown-linux-musl` on the
Rust side.

**Q: The binary is still too large?**
A: Trim it as needed: `[profile.release]` already enables
LTO/`opt-level=z`/`panic=abort`/`strip`. If it still exceeds the gate, consider
nightly `-Z build-std` with `panic_immediate_abort` (not enabled in CI yet).

**Q: Why a separate `openwrt-packages.yml`?**
A: Because it must build the rustc bootstrap from source inside the SDK
(30–90 minutes per architecture), which does not belong on the per-commit
pipeline. The daily CI already proves "musl buildable + correct architecture"
via path A.

**Q: Where does relay mode show up?**
A: See the "Wi-Fi & KVR" page description in [`FRONTEND.md`](FRONTEND.md) and M6 in
[`BACKEND.md`](BACKEND.md); it is implemented as `wpa_supplicant(STA)` + `hostapd(AP)` with
**no mesh (802.11s) code path at all**.
