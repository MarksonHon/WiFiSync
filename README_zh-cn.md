[English](README.md) | **简体中文**

# WifiSync

面向 **OpenWrt** 的家庭局域网（含无线）组网工具。Rust 实现，单二进制，带 LuCI 界面。

* **零侵入** —— Gateway / Controller 不产生任何网络配置写入，只有纯 AP 才应用改动。
* **可回退** —— 启动前保存初始化基线，停止时恢复原有网络。
* **最小职责** —— Controller 验证并下发，Gateway 标识并探测，AP 应用并上报。
* KVR（802.11k/v/r）由 Controller 统一下发；**不实现 mesh**，Wi-Fi 中继使用
  `wpa_supplicant(STA)` 上行 + `hostapd(AP)` 下行。
* 可选故障恢复：apply-guard 回滚、AP 链路看门狗、procd 自愈。

## 支持平台

OpenWrt 25.12，x86 与 ARM64（7 种包架构）。见 [`docs/BUILDING_zh-cn.md`](docs/BUILDING_zh-cn.md)。

## 安装

```sh
# 在 OpenWrt 构建树里把本仓库加为 feed
echo "src-link wifisync /path/to/WifiSync" >> feeds.conf.default
./scripts/feeds update wifisync && ./scripts/feeds install -a -p wifisync
make menuconfig      # Network → wifisync / LuCI → Applications → luci-app-wifisync
make package/wifisync/compile V=s
```

## 使用

```sh
wifisync status                     # 看能力与默认角色
wifisync plan                       # 看会做哪些改动（非 AP 角色恒为 0 项）
wifisync apply && wifisync confirm  # 只有纯 AP 才会真的写
wifisync backup list
wifisync backup verify
wifisync restore                    # 默认 managed_only
wifisync restore --full             # 按基线整体覆盖
wifisync account add <name>         # 为远程 AP / Gateway 创建 Controller 账号
wifisync link                       # 到 Controller 的连接（默认端口 6550）
wifisync lan report                 # Gateway 上报的 LAN 网桥 / 网络 / DHCP / IPv6
```

LuCI：**服务 → WifiSync**，共 8 个页面：状态总览 / 角色 / Gateway / Controller / 网桥 / Wi-Fi 与 KVR / 备份与故障恢复 / 诊断。

## 文档

| 文档 | 内容 |
|------|------|
| [`docs/BACKEND_zh-cn.md`](docs/BACKEND_zh-cn.md) | 后端设计：需求追溯、角色模型、依赖、生命周期、里程碑、测试（[English](docs/BACKEND.md)） |
| [`docs/FRONTEND_zh-cn.md`](docs/FRONTEND_zh-cn.md) | 前端设计：ubus 契约、页面清单、安全交互、多语言（[English](docs/FRONTEND.md)） |
| [`docs/BUILDING_zh-cn.md`](docs/BUILDING_zh-cn.md) | 构建路径、架构映射、CI（[English](docs/BUILDING.md)） |
| [`docs/STEERING_zh-cn.md`](docs/STEERING_zh-cn.md) | 客户端引导设计（**提案，尚未实现**）：Controller 中心架构、协议、安全护栏、里程碑（[English](docs/STEERING.md)） |

## 许可

GPL-2.0-only，见 [`LICENSE`](LICENSE)。
