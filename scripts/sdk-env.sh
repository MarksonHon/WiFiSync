#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# Download and extract the **official OpenWrt SDK**, exporting its bundled musl cross toolchain into the environment.
#
# This is the most direct way to "reuse the OpenWrt toolchain": the linker is the one in the SDK,
# `staging_dir/toolchain-*/bin/*-musl-gcc`, exactly as in official package builds.
#
# Usage:
#   . scripts/sdk-env.sh <target/subtarget>          # e.g. x86/64, mediatek/filogic
#
# Environment variables:
#   WIFISYNC_OPENWRT_RELEASE  default 25.12.5
#   WIFISYNC_SDK_DIR          default $PWD/.sdk
#
# Exports (used by build-musl.sh / cargo):
#   WIFISYNC_CC      cross gcc (also used as cargo's linker)
#   WIFISYNC_SDK     SDK root directory
#   PATH             toolchain bin directory appended

set -eu

TARGET_PATH="$1"
REL="${WIFISYNC_OPENWRT_RELEASE:-25.12.5}"
SDK_ROOT="${WIFISYNC_SDK_DIR:-$PWD/.sdk}"
BASE="https://downloads.openwrt.org/releases/${REL}/targets/${TARGET_PATH}/"

mkdir -p "$SDK_ROOT"

# 1. Resolve the SDK filename from the official directory index (the toolchain minor version changes between releases)
SDK_NAME=$(curl -fsSL "$BASE" | grep -o 'openwrt-sdk-[^"]*Linux-x86_64\.tar\.zst' | head -n1)
if [ -z "$SDK_NAME" ]; then
	echo "error: no SDK archive found in $BASE (check the OpenWrt release and target path)" >&2
	exit 1
fi

# 2. Download (with caching)
if [ ! -f "$SDK_ROOT/$SDK_NAME" ]; then
	echo "downloading $SDK_NAME ..."
	curl -fL --retry 3 -o "$SDK_ROOT/$SDK_NAME.part" "$BASE$SDK_NAME"
	mv "$SDK_ROOT/$SDK_NAME.part" "$SDK_ROOT/$SDK_NAME"
fi

# 3. Extract (only once)
SDK_DIR="$SDK_ROOT/$(basename "$SDK_NAME" .tar.zst)"
if [ ! -d "$SDK_DIR" ]; then
	echo "extracting $SDK_NAME ..."
	if tar --zstd -tf "$SDK_ROOT/$SDK_NAME" >/dev/null 2>&1; then
		tar --zstd -xf "$SDK_ROOT/$SDK_NAME" -C "$SDK_ROOT"
	elif command -v zstd >/dev/null 2>&1; then
		zstd -d -c "$SDK_ROOT/$SDK_NAME" | tar -xf - -C "$SDK_ROOT"
	else
		echo "error: zstd is required (apt-get install zstd)" >&2
		exit 1
	fi
fi

# 4. Locate the toolchain
TOOLCHAIN_BIN=$(find "$SDK_DIR/staging_dir" -maxdepth 2 -type d -name 'toolchain-*' -print -quit)/bin
if [ ! -d "$TOOLCHAIN_BIN" ]; then
	echo "error: no toolchain directory in the SDK ($SDK_DIR/staging_dir/toolchain-*)" >&2
	exit 1
fi

CC=$(find "$TOOLCHAIN_BIN" -maxdepth 1 -name '*-musl-gcc' -print -quit)
if [ -z "$CC" ]; then
	echo "error: no *-musl-gcc found in $TOOLCHAIN_BIN" >&2
	exit 1
fi

WIFISYNC_CC="$CC"
WIFISYNC_SDK="$SDK_DIR"
PATH="$TOOLCHAIN_BIN:$PATH"
export WIFISYNC_CC WIFISYNC_SDK PATH

echo "OK: SDK=$WIFISYNC_SDK"
echo "OK: CC=$WIFISYNC_CC"
