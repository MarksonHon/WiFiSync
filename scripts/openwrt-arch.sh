#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
#
# OpenWrt 架构 ↔ 官方 SDK 目标 ↔ Rust target 三元组 的**唯一**映射表。
#
# 依据 OpenWrt 25.12（downloads.openwrt.org/releases/25.12.0/packages/ 中的
# 包架构目录 + openwrt-25.12 分支各 target 的 target.mk 里的 CPU_TYPE）：
#
#   包架构              SDK target/subtarget   CPU_TYPE
#   x86_64              x86/64                 x86_64
#   i386_pentium4       x86/generic            pentium4
#   i386_pentium-mmx    x86/legacy             pentium-mmx
#   aarch64_generic     armsr/armv8            (generic)
#   aarch64_cortex-a53  mediatek/filogic       cortex-a53
#   aarch64_cortex-a72  bcm27xx/bcm2711        cortex-a72
#   aarch64_cortex-a76  bcm27xx/bcm2712        cortex-a76
#
# 用法:
#   scripts/openwrt-arch.sh list                 # 列出全部支持的包架构
#   scripts/openwrt-arch.sh <arch> target        # x86/64
#   scripts/openwrt-arch.sh <arch> triple        # x86_64-unknown-linux-musl
#   scripts/openwrt-arch.sh <arch> cpu           # 传给 rustc -C target-cpu
#   scripts/openwrt-arch.sh <arch> all

set -eu

ARCH_LIST="x86_64 i386_pentium4 i386_pentium-mmx aarch64_generic aarch64_cortex-a53 aarch64_cortex-a72 aarch64_cortex-a76"

arch="$1"
what="${2:-all}"

case "$arch" in
	list) echo "$ARCH_LIST"; exit 0 ;;
esac

case "$arch" in
	x86_64)             target=x86/64;          triple=x86_64-unknown-linux-musl;  cpu=generic ;;
	i386_pentium4)      target=x86/generic;     triple=i686-unknown-linux-musl;    cpu=pentium4 ;;
	i386_pentium-mmx)   target=x86/legacy;      triple=i586-unknown-linux-musl;    cpu=pentium-mmx ;;
	aarch64_generic)    target=armsr/armv8;     triple=aarch64-unknown-linux-musl; cpu=generic ;;
	aarch64_cortex-a53) target=mediatek/filogic; triple=aarch64-unknown-linux-musl; cpu=cortex-a53 ;;
	aarch64_cortex-a72) target=bcm27xx/bcm2711;  triple=aarch64-unknown-linux-musl; cpu=cortex-a72 ;;
	aarch64_cortex-a76) target=bcm27xx/bcm2712;  triple=aarch64-unknown-linux-musl; cpu=cortex-a76 ;;
	*)
		echo "错误：未知的 OpenWrt 包架构 \`$arch\`" >&2
		echo "可用：$ARCH_LIST" >&2
		exit 1
		;;
esac

case "$what" in
	target) echo "$target" ;;
	triple) echo "$triple" ;;
	cpu) echo "$cpu" ;;
	all) echo "$target $triple $cpu" ;;
	*) echo "错误：未知字段 \`$what\`" >&2; exit 1 ;;
esac
