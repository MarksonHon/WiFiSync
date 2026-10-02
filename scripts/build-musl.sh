#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# 用 OpenWrt SDK 的 musl 工具链交叉编译 wifisync 单二进制。
#
# 与 OpenWrt 官方 Rust 包（feeds/packages/lang/rust/rust-values.mk）保持一致的要点：
#   * 链接器 = OpenWrt 工具链的 *-musl-gcc
#   * -C target-feature=-crt-static（动态链接设备上的 musl，体积更小）
#   * --locked（Cargo.lock 固定）
#
# 用法:
#   scripts/build-musl.sh <openwrt-arch>          # 如 aarch64_cortex-a53
#   WIFISYNC_SKIP_SDK=1 scripts/build-musl.sh ... # 复用已有 SDK（CI 缓存场景）

set -eu

ARCH="$1"
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

TARGET=$(scripts/openwrt-arch.sh "$ARCH" target)
TRIPLE=$(scripts/openwrt-arch.sh "$ARCH" triple)
CPU=$(scripts/openwrt-arch.sh "$ARCH" cpu)

echo "== 架构 $ARCH → target $TARGET / triple $TRIPLE / cpu $CPU"

# 1. 准备 SDK 工具链
# shellcheck disable=SC1091
. scripts/sdk-env.sh "$TARGET"

# 2. 确保 rust target 可用
if command -v rustup >/dev/null 2>&1; then
	rustup target add "$TRIPLE" >/dev/null 2>&1 || true
fi

# 3. cargo 需要的环境
TRIPLE_UPPER=$(echo "$TRIPLE" | tr 'a-z-' 'A-Z_')
LINKER_VAR="CARGO_TARGET_${TRIPLE_UPPER}_LINKER"
export "$LINKER_VAR=$WIFISYNC_CC"

# 与 rust-values.mk 一致的 RUSTFLAGS：动态链接 musl + 目标 CPU
RUSTFLAGS="-C target-cpu=$CPU -C target-feature=-crt-static"
export RUSTFLAGS

# 体积与 LTO 参数与 rust-values.mk 对齐
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_DEBUG=false
export CARGO_PROFILE_RELEASE_LTO=true
export CARGO_PROFILE_RELEASE_OPT_LEVEL=z

echo "== 开始编译（linker=$WIFISYNC_CC）"
cargo build --release --locked --target "$TRIPLE" -p wifisync

OUT="target/$TRIPLE/release/wifisync"
if [ ! -f "$OUT" ]; then
	echo "错误：没有生成 $OUT" >&2
	exit 1
fi

# 4. 记录产物信息（CI 上传、体积门禁都用它）
DIST="$ROOT/dist/$ARCH"
mkdir -p "$DIST"
cp "$OUT" "$DIST/wifisync"
# SDK 里的 strip 未必支持全部架构，用 host 的 strip 只在能识别时执行
if command -v strip >/dev/null 2>&1; then
	strip "$DIST/wifisync" 2>/dev/null || true
fi

SIZE=$(wc -c < "$DIST/wifisync")
FILE_INFO=$(file -b "$DIST/wifisync" 2>/dev/null || echo "unknown")

cat > "$DIST/build-info.json" <<EOF
{
  "openwrt_arch": "$ARCH",
  "openwrt_target": "$TARGET",
  "rust_triple": "$TRIPLE",
  "rust_target_cpu": "$CPU",
  "linker": "$WIFISYNC_CC",
  "size_bytes": $SIZE,
  "file": "$FILE_INFO",
  "openwrt_release": "${WIFISYNC_OPENWRT_RELEASE:-25.12.5}"
}
EOF

echo "== 产物 $DIST/wifisync（$SIZE 字节）"
echo "   $FILE_INFO"
