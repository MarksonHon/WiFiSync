#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Cross-compile the wifisync single binary with the OpenWrt SDK musl toolchain.
#
# Points kept consistent with the OpenWrt official Rust package (feeds/packages/lang/rust/rust-values.mk):
#   * linker = the OpenWrt toolchain's *-musl-gcc
#   * -C target-feature=-crt-static (dynamically link the device's musl, smaller size)
#   * --locked (pin Cargo.lock)
#
# Usage:
#   scripts/build-musl.sh <openwrt-arch>          # e.g. aarch64_cortex-a53
#   WIFISYNC_SKIP_SDK=1 scripts/build-musl.sh ... # reuse an existing SDK (CI cache scenario)

set -eu

ARCH="$1"
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

TARGET=$(scripts/openwrt-arch.sh "$ARCH" target)
TRIPLE=$(scripts/openwrt-arch.sh "$ARCH" triple)
CPU=$(scripts/openwrt-arch.sh "$ARCH" cpu)

echo "== arch $ARCH -> target $TARGET / triple $TRIPLE / cpu $CPU"

# 1. Prepare the SDK toolchain
# shellcheck disable=SC1091
. scripts/sdk-env.sh "$TARGET"

# 2. Make sure the rust target is available
if command -v rustup >/dev/null 2>&1; then
	rustup target add "$TRIPLE" >/dev/null 2>&1 || true
fi

# 3. Environment needed by cargo
TRIPLE_UPPER=$(echo "$TRIPLE" | tr 'a-z-' 'A-Z_')
LINKER_VAR="CARGO_TARGET_${TRIPLE_UPPER}_LINKER"
export "$LINKER_VAR=$WIFISYNC_CC"

# RUSTFLAGS matching rust-values.mk: dynamically link musl + target CPU
RUSTFLAGS="-C target-cpu=$CPU -C target-feature=-crt-static"
export RUSTFLAGS

# Size and LTO settings aligned with rust-values.mk
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_DEBUG=false
export CARGO_PROFILE_RELEASE_LTO=true
export CARGO_PROFILE_RELEASE_OPT_LEVEL=z

echo "== building (linker=$WIFISYNC_CC)"
cargo build --release --locked --target "$TRIPLE" -p wifisync

OUT="target/$TRIPLE/release/wifisync"
if [ ! -f "$OUT" ]; then
	echo "error: $OUT was not produced" >&2
	exit 1
fi

# 4. Record artifact info (used by CI upload and the size gate)
DIST="$ROOT/dist/$ARCH"
mkdir -p "$DIST"
cp "$OUT" "$DIST/wifisync"
# The SDK's strip may not support every architecture, so use the host strip only when it recognizes the file
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

echo "== artifact $DIST/wifisync ($SIZE bytes)"
echo "   $FILE_INFO"
