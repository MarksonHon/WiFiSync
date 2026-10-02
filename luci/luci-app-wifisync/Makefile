# SPDX-License-Identifier: GPL-2.0-only
#
# LuCI 应用：WifiSync 管理界面
#
# 依赖 LuCI 的 feed：include $(TOPDIR)/feeds/luci/luci.mk
# 界面全部通过 ubus 对象 `wifisync`（由 rpcd exec 插件桥接）与后端通信。

include $(TOPDIR)/rules.mk

LUCI_TITLE:=LuCI app for WifiSync (家庭局域网组网)
LUCI_DESCRIPTION:=WifiSync 管理界面：角色、网桥、Wi-Fi/KVR、准入、备份与故障恢复
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
