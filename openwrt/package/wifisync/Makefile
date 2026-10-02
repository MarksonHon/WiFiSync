# SPDX-License-Identifier: GPL-2.0-only
#
# WifiSync —— OpenWrt 包定义
#
# 构建方式完全沿用 OpenWrt 官方 Rust 包的做法：
#   * 依赖 `rust/host`（feeds/packages/lang/rust）
#   * include rust-package.mk，使用 `cargo install --root $(PKG_INSTALL_DIR)`
#   * 架构约束复用 `RUST_ARCH_DEPENDS`
#
# 源码就在本 feed 内，由 Build/Prepare 从仓库根目录复制（无需 PKG_SOURCE）。

include $(TOPDIR)/rules.mk

PKG_NAME:=wifisync
PKG_VERSION:=0.1.0
PKG_RELEASE:=1

PKG_LICENSE:=GPL-2.0-only
PKG_LICENSE_FILES:=LICENSE
PKG_MAINTAINER:=WifiSync contributors

PKG_BUILD_DEPENDS:=rust/host
PKG_BUILD_PARALLEL:=1
PKG_BUILD_FLAGS:=no-mips16

include $(INCLUDE_DIR)/package.mk
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

# 仓库根目录（本包位于 <repo>/openwrt/package/wifisync）
REPO_ROOT:=$(realpath $(CURDIR)/../../..)

define Package/wifisync
  SECTION:=net
  CATEGORY:=Network
  TITLE:=WifiSync — 家庭局域网（含有线+无线）组网工具
  URL:=https://github.com/openwrt-wifisync/wifisync
  DEPENDS:=$(RUST_ARCH_DEPENDS) +wpad +kmod-br-netfilter
endef

define Package/wifisync/description
  WifiSync 用「Controller 准入 + 网络信息下发 + 802.11k/v/r 漫游 + 标准 Wi-Fi 中继」
  组建家庭单网。
  设计原则：默认零侵入 —— Gateway/Controller 不修改任何既有网络配置，
  只有「纯 AP」设备（有 AP 且无 Controller、无 Gateway）才会把所有网口并入 br-lan。
  服务启动前保存初始化网络基线，停止前恢复原有网络。
  注意：802.11k/v/r 需要完整版 wpad（wpad-basic-mbedtls 不含），缺失时程序会拒绝开启 KVR。
endef

define Build/Prepare
	rm -rf $(PKG_BUILD_DIR)
	$(INSTALL_DIR) $(PKG_BUILD_DIR)
	$(CP) $(REPO_ROOT)/Cargo.toml $(PKG_BUILD_DIR)/
	$(CP) $(REPO_ROOT)/Cargo.lock $(PKG_BUILD_DIR)/
	$(CP) $(REPO_ROOT)/crates $(PKG_BUILD_DIR)/
	$(CP) $(REPO_ROOT)/LICENSE $(PKG_BUILD_DIR)/ 2>/dev/null || true
endef

# 单二进制：daemon / CLI / rpcd 插件三种模式
define Build/Compile
	$(call Build/Compile/Cargo,crates/wifisync)
endef

define Package/wifisync/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/wifisync $(1)/usr/bin/wifisync

	$(INSTALL_DIR) $(1)/etc/init.d $(1)/etc/config
	$(INSTALL_BIN) ./files/etc/init.d/wifisync $(1)/etc/init.d/wifisync
	$(INSTALL_CONF) ./files/etc/config/wifisync $(1)/etc/config/wifisync

	$(INSTALL_DIR) $(1)/usr/libexec/rpcd $(1)/usr/share/rpcd/acl.d
	$(INSTALL_BIN) ./files/usr/libexec/rpcd/wifisync $(1)/usr/libexec/rpcd/wifisync
	$(INSTALL_DATA) ./files/usr/share/rpcd/acl.d/wifisync.json $(1)/usr/share/rpcd/acl.d/wifisync.json
endef

define Package/wifisync/conffiles
/etc/config/wifisync
endef

# 卸载前先恢复网络，避免设备被留在半配置状态
define Package/wifisync/prerm
	#!/bin/sh
	[ -x "$${IPKG_INSTROOT}/usr/bin/wifisync" ] || exit 0
	$${IPKG_INSTROOT}/etc/init.d/wifisync stop >/dev/null 2>&1 || true
	exit 0
endef

$(eval $(call BuildPackage,wifisync))
