#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# 下载并解包 **OpenWrt 官方 SDK**，把它自带的 musl 交叉工具链导出到环境里。
#
# 这是"复用 OpenWrt 工具链"的最直接方式：链接器就是 SDK 里那份
# `staging_dir/toolchain-*/bin/*-musl-gcc`，与官方包构建完全一致。
#
# 用法:
#   . scripts/sdk-env.sh <target/subtarget>          # 如 x86/64、mediatek/filogic
#
# 可用环境变量:
#   WIFISYNC_OPENWRT_RELEASE  默认 25.12.5
#   WIFISYNC_SDK_DIR          默认 $PWD/.sdk
#
# 导出（供 build-musl.sh / cargo 使用）:
#   WIFISYNC_CC      交叉 gcc（同时用作 cargo 的 linker）
#   WIFISYNC_SDK     SDK 根目录
#   PATH             已加入工具链 bin 目录

set -eu

TARGET_PATH="$1"
REL="${WIFISYNC_OPENWRT_RELEASE:-25.12.5}"
SDK_ROOT="${WIFISYNC_SDK_DIR:-$PWD/.sdk}"
BASE="https://downloads.openwrt.org/releases/${REL}/targets/${TARGET_PATH}/"

mkdir -p "$SDK_ROOT"

# 1. 从官方目录索引里解析出 SDK 文件名（不同发布版本工具链小版本号会变）
SDK_NAME=$(curl -fsSL "$BASE" | grep -o 'openwrt-sdk-[^"]*Linux-x86_64\.tar\.zst' | head -n1)
if [ -z "$SDK_NAME" ]; then
	echo "错误：在 $BASE 未找到 SDK 压缩包（检查 OpenWrt 版本与 target 路径）" >&2
	exit 1
fi

# 2. 下载（带缓存）
if [ ! -f "$SDK_ROOT/$SDK_NAME" ]; then
	echo "下载 $SDK_NAME ..."
	curl -fL --retry 3 -o "$SDK_ROOT/$SDK_NAME.part" "$BASE$SDK_NAME"
	mv "$SDK_ROOT/$SDK_NAME.part" "$SDK_ROOT/$SDK_NAME"
fi

# 3. 解包（只解一次）
SDK_DIR="$SDK_ROOT/$(basename "$SDK_NAME" .tar.zst)"
if [ ! -d "$SDK_DIR" ]; then
	echo "解包 $SDK_NAME ..."
	if tar --zstd -tf "$SDK_ROOT/$SDK_NAME" >/dev/null 2>&1; then
		tar --zstd -xf "$SDK_ROOT/$SDK_NAME" -C "$SDK_ROOT"
	elif command -v zstd >/dev/null 2>&1; then
		zstd -d -c "$SDK_ROOT/$SDK_NAME" | tar -xf - -C "$SDK_ROOT"
	else
		echo "错误：需要 zstd（apt-get install zstd）" >&2
		exit 1
	fi
fi

# 4. 定位工具链
TOOLCHAIN_BIN=$(find "$SDK_DIR/staging_dir" -maxdepth 2 -type d -name 'toolchain-*' -print -quit)/bin
if [ ! -d "$TOOLCHAIN_BIN" ]; then
	echo "错误：在 SDK 中找不到工具链目录（$SDK_DIR/staging_dir/toolchain-*）" >&2
	exit 1
fi

CC=$(find "$TOOLCHAIN_BIN" -maxdepth 1 -name '*-musl-gcc' -print -quit)
if [ -z "$CC" ]; then
	echo "错误：在 $TOOLCHAIN_BIN 找不到 *-musl-gcc" >&2
	exit 1
fi

WIFISYNC_CC="$CC"
WIFISYNC_SDK="$SDK_DIR"
PATH="$TOOLCHAIN_BIN:$PATH"
export WIFISYNC_CC WIFISYNC_SDK PATH

echo "OK: SDK=$WIFISYNC_SDK"
echo "OK: CC=$WIFISYNC_CC"
