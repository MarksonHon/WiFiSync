# SPDX-License-Identifier: GPL-2.0-only
#
# WiFiSync - OpenWrt package definition
#
# The build follows the OpenWrt official Rust package approach exactly:
#   * depends on `rust/host` (feeds/packages/lang/rust)
#   * includes rust-package.mk and uses `cargo install --root $(PKG_INSTALL_DIR)`
#   * reuses `RUST_ARCH_DEPENDS` for architecture constraints
#
# Sources live inside this feed; Build/Prepare copies them from the repository root (no PKG_SOURCE needed).

include $(TOPDIR)/rules.mk

PKG_NAME:=wifisync
PKG_VERSION:=0.1.0
PKG_RELEASE:=1

PKG_LICENSE:=GPL-2.0-only
PKG_LICENSE_FILES:=LICENSE
PKG_MAINTAINER:=WiFiSync contributors

PKG_BUILD_DEPENDS:=rust/host
PKG_BUILD_PARALLEL:=1
PKG_BUILD_FLAGS:=no-mips16

include $(INCLUDE_DIR)/package.mk
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

# Repository root (this package is at <repo>/openwrt/package/wifisync)
REPO_ROOT:=$(realpath $(CURDIR)/../../..)

define Package/wifisync
  SECTION:=net
  CATEGORY:=Network
  TITLE:=WiFiSync — home LAN (wired + wireless) networking tool
  URL:=https://github.com/openwrt-wifisync/wifisync
  DEPENDS:=$(RUST_ARCH_DEPENDS) +wpad +kmod-br-netfilter
endef

define Package/wifisync/description
  WiFiSync builds a single home network from "Controller admission + network information
  distribution + 802.11k/v/r roaming + standard Wi-Fi relay".
  Design principle: zero intrusion by default — the Gateway/Controller roles never modify any
  existing network configuration; only a "pure AP" device (AP without Controller and Gateway)
  merges all ports into br-lan.
  The initial network baseline is saved before the service starts and restored before it stops.
  Note: 802.11k/v/r requires the full wpad package (wpad-basic-mbedtls does not include it); the
  program refuses to enable KVR when it is missing.
endef

define Build/Prepare
	rm -rf $(PKG_BUILD_DIR)
	$(INSTALL_DIR) $(PKG_BUILD_DIR)
	$(CP) $(REPO_ROOT)/Cargo.toml $(PKG_BUILD_DIR)/
	$(CP) $(REPO_ROOT)/Cargo.lock $(PKG_BUILD_DIR)/
	$(CP) $(REPO_ROOT)/crates $(PKG_BUILD_DIR)/
	$(CP) $(REPO_ROOT)/LICENSE $(PKG_BUILD_DIR)/ 2>/dev/null || true
endef

# Single binary: daemon / CLI / rpcd plugin, three modes
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

# Restore the network before uninstall to avoid leaving the device half-configured
define Package/wifisync/prerm
	#!/bin/sh
	[ -x "$${IPKG_INSTROOT}/usr/bin/wifisync" ] || exit 0
	$${IPKG_INSTROOT}/etc/init.d/wifisync stop >/dev/null 2>&1 || true
	exit 0
endef

$(eval $(call BuildPackage,wifisync))
