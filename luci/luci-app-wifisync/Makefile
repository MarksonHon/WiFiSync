# SPDX-License-Identifier: GPL-2.0-only
#
# LuCI application: WifiSync management interface
#
# Requires the LuCI feed: include $(TOPDIR)/feeds/luci/luci.mk
# All views talk to the backend through the ubus object `wifisync`, which is
# bridged by the rpcd exec plugin. Translations are picked up automatically
# from the po/ directory and shipped as separate luci-i18n-wifisync-<lang>
# packages.

include $(TOPDIR)/rules.mk

LUCI_TITLE:=LuCI app for WifiSync (home LAN networking)
LUCI_DESCRIPTION:=WifiSync management interface: roles, bridge, Wi-Fi/KVR, admission, backup and failover
LUCI_DEPENDS:=+wifisync +luci-base
LUCI_PKGARCH:=all

PKG_NAME:=luci-app-wifisync
PKG_VERSION:=0.1.0
PKG_RELEASE:=1
PKG_LICENSE:=GPL-2.0-only
PKG_MAINTAINER:=WifiSync contributors

include $(TOPDIR)/feeds/luci/luci.mk

# call BuildPackage - OpenWrt buildroot signature
$(eval $(call BuildPackage,luci-app-wifisync))
