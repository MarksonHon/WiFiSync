[English](FRONTEND.md) | **简体中文**

# 前端（LuCI）设计

> 范围：`luci/luci-app-wifisync`。界面是**纯 Web 界面**：自身不落地任何网络配置，所有动作都经
> `wifisync` 后端完成。

后端的角色模型、备份恢复、故障恢复见 [`BACKEND_zh-cn.md`](BACKEND_zh-cn.md)；构建与打包见
[`BUILDING_zh-cn.md`](BUILDING_zh-cn.md)。

---

## 1. 设计约束

1. **不直接改网络配置**：LuCI 不得包含任何直接同步网络配置的代码，功能一律由后端服务
   （`wifisync`）承担，见 `AGENTS.md`。
2. **零侵入必须可见**：可能改动本机网络的页面要明确说明是否会写入、写入什么，风险动作需
   二次确认 + dry-run 预览。
3. **不引入新协议**：只复用 LuCI 标准机制 —— ubus + `rpc.declare()`。
4. **不写死语言**：界面文案必须走 LuCI 的翻译机制，并以语言包形式随用户语言偏好动态加载，
   而不是硬编码在视图代码里。

---

## 2. 通信契约

```
LuCI JS 视图  →  ubus 对象 `wifisync`  →  rpcd exec 插件 (/usr/libexec/rpcd/wifisync)
             →  UNIX socket /var/run/wifisync/wifisync.sock (0600)  →  wifisync 守护进程
```

* 视图只调用 `rpc.declare({ object: 'wifisync', method: '…' })`，自身**不读写** `/etc/config`。
* 权限由 `root/usr/share/rpcd/acl.d/luci-app-wifisync.json` 授予，并且**读写分组**：

| 权限 | 方法 |
|------|------|
| read | `status`、`capabilities`、`roles_get`、`bridge_preview`、`plan_dry_run`、`wifi_source_get`、`admission_list`、`backup_list`、`backup_verify`、`failsafe_get`、`profile_get`、`link_get`、`link_status`、`account_list`、`lan_report`、`lan_list`、`logs_tail`、`version` |
| write | `roles_set`、`apply`、`confirm`、`revert_last_change`、`restore`、`wifi_source_set`、`gateway_set`、`probe_connectivity`、`admission_register`、`admission_approve`、`admission_reject`、`admission_revoke`、`backup_create`、`backup_prune`、`failsafe_set`、`profile_publish`、`link_set`、`account_add`、`account_passwd`、`account_remove` |

两部分同时授予 `uci: [ "wifisync" ]`（仅服务自身配置）。

---

## 3. 文件结构

```
luci/luci-app-wifisync/
├─ Makefile                                  # 基于 luci.mk；LUCI_DEPENDS := +wifisync +luci-base
├─ htdocs/luci-static/resources/view/wifisync/
│  ├─ common.js        # 全部 rpc.declare() 声明、MESSAGES 消息表与公共控件
│  ├─ overview.js      # 状态总览
│  ├─ roles.js         # 角色向导
│  ├─ gateway.js       # LAN 接口选择（零侵入）
│  ├─ controller.js    # 准入列表、Wi-Fi 来源、连通性
│  ├─ bridge.js        # 建桥规划预览（仅纯 AP）
│  ├─ wifi.js          # Wi-Fi 与 KVR
│  ├─ backup.js        # 备份、恢复与故障恢复
│  └─ diagnostics.js   # 日志、dry-run、诊断包
├─ po/zh_Hans/luci-app-wifisync.po           # 翻译词条（luci.mk 生成 luci-i18n-wifisync-zh-cn 包）
└─ root/usr/share/
   ├─ luci/menu.d/luci-app-wifisync.json     # 菜单注册（`admin/services/wifisync`，受 ACL 约束）
   └─ rpcd/acl.d/luci-app-wifisync.json      # ubus 方法读写白名单
```

`common.js` 是 ubus 方法面的**唯一**声明处，后端改方法名只需改这一处。

---

## 4. 页面清单

菜单位置：**服务 → WifiSync**（`admin/services/wifisync`）。

| 排序 | 页面 | 内容 |
|------|------|------|
| 10 | 状态总览 | 角色徽标、无线存在性、**Controller↔Gateway 连通性徽标**、**待准入 AP 数**、网桥成员、心跳/回滚倒计时、**基线状态与最近一次恢复时间** |
| 20 | 角色 | 三复选框；无无线时 AP 禁用并提示；显式标注「Gateway/Controller 不会修改你的网络配置」；**到 Controller 的连接**（地址、账号、密码、连接状态——密码只写不读） |
| 30 | Gateway | 仅「选择对应 LAN 接口」+ 探针目标；**Gateway 上报给 Controller 的 LAN 信息**（网桥、网络、DHCP 范围、IPv6 策略，只读）；页面顶部常驻零侵入说明；**无任何网络写入控件** |
| 40 | Controller | ① **准入列表**（待批准/已批准/黑名单，批准/拒绝/撤销）② **Wi-Fi 信息源三选一**（自身/Gateway/自定义，含禁用原因提示）③ `NetworkProfile` 预览与下发按钮 ④ 连通性探测目标与「确认连通」按钮 ⑤ **监听设置**（监听地址与端口，默认 6550）⑥ AP/Gateway 的**账号**（创建/改密/删除）⑦ **Gateway 上报的 LAN 信息** |
| 50 | 网桥 | **仅纯 AP 设备可见**；自动规划预览 + dry-run diff；非纯 AP 时显示「当前角色组合不建桥」及原因；预留多网桥/VLAN 高级区（默认折叠） |
| 60 | Wi-Fi 与 KVR | `mobility_domain`、FT 模式、k/v 开关；`wpad` 版本检查与安装引导 |
| 70 | 备份与故障恢复 | 基线信息（时间/校验和/`managed_keys`）、快照列表（pre-start / pre-change）、完整性校验、`restore_mode` 选择、**「立即恢复基线」**、**「重新建立基线」**（二次确认 + dry-run 预览）、停止时是否恢复的逃生开关，以及故障恢复开关、超时、动作与手动回滚（仅 AP 侧生效，页面注明） |
| 80 | 诊断 | 日志尾部、一键 dry-run 对比、同步版本号与 diff、**备份完整性报告** |

> 上述页面清单是需求 R1 的前端落点；R3、R5、R6、R9、R11 中与界面相关的部分（禁用、可见性、
> 二次确认）见 §5 与 §6。

---

## 5. 交互与安全模式

| 模式 | 出现位置 | 行为 |
|------|---------|------|
| 零侵入提示 | 状态总览、角色、Gateway、网桥 | 说明 Gateway/Controller 角色不产生本机网络写入；网桥页说明只有纯 AP 才建桥 |
| 禁用并给出原因 | Wi-Fi 来源选择、AP 复选框、网桥页 | 控件置灰且**写明原因**（如「Gateway 上报无 Wi-Fi」「本机无无线」），而不是静默消失 |
| dry-run 预览 | 状态总览（`plan_dry_run`）、网桥（`bridge_preview`） | 写入前展示 uci/网络 diff；非 AP 角色预览恒为空 |
| 应用 + 确认 | 状态总览 | `apply` 会武装 apply-guard 死手定时器；用户必须在超时内点击 `confirm`，否则自动回滚 |
| 二次确认 | 同设备 `custom` 改 Wi-Fi、`full` 恢复、重建基线、停止时不恢复 | 额外弹窗确认 + 操作前预览 |
| 徽标 | 状态总览、Controller、备份 | 连通性「未确认/已确认」、待准入数量、基线有效性、最近恢复时间 |

---

## 6. 角色与能力门控

| 条件 | 可见 / 可用 |
|------|------------|
| 无无线（`/sys/class/ieee80211/*` 为空） | AP 复选框禁用；默认角色 `controller + gateway`；网桥页显示「不建桥」 |
| 非纯 AP | 网桥页显示空计划与原因，无逐端口控件 |
| 纯 AP（`ap && !gateway && !controller`） | 网桥页显示规划端口列表与 dry-run diff |
| Controller 角色 | 准入列表、Wi-Fi 来源选择、Profile 下发、连通性确认 |
| Gateway 角色 | 仅 LAN 接口选择与探针目标 |
| AP 角色 | Wi-Fi/KVR 页与 apply/confirm、revert 控件有意义；备份/恢复在所有设备均可用 |

---

## 7. 多语言（语言包）

* 界面文案不得写死：视图中所有面向用户的字符串都用 LuCI 的 `_()` 翻译函数包裹，英文原文即
  msgid（源语言为英文）。
* 翻译以独立的 LuCI 语言包形式发布：词条放在 `po/<lang>/luci-app-wifisync.po`，`luci.mk` 会自动
  发现 `po/*` 并生成 `luci-i18n-wifisync-<lang>` 包（如 `po/zh_Hans/` →
  `luci-i18n-wifisync-zh-cn`），安装为 `/usr/lib/lua/luci/i18n/luci-app-wifisync.<lang>.lmo`；
  由 `luci.languages` 设置选择，界面随用户语言偏好切换，无需重新构建应用。
* `menu.d` 的标题同样使用英文源串，由 LuCI 通过同一份词条翻译。
* 当前状态：已提供 `zh_Hans`；新增语言只需新增 `po/<lang>/luci-app-wifisync.po`。
* **后端文案也不预渲染**：`plan.notes`、`bridge_blocked_reason`、`failsafe.state_label`、Wi-Fi
  来源禁用原因、角色调整提示、备份/恢复摘要、apply/gateway 提示、探测详情等，均以结构化消息
  `{ key, params }` 形式下发（见 `wifisync-core::message`）。`common.js` 中的 `MESSAGES` 表把每个
  key 映射为 `_('英文模板，含 %{param}')`，由 `ws.message()`、`ws.planText()`、`ws.errorText()`
  渲染；两侧的 key 集合需在改动时任一侧时手工核对（属评审步骤，不再由 CI 自动比对）。
* CLI 与服务日志保持**纯英文**、不参与翻译 —— 本地化只发生在这里的视图中，因此诊断页会直接
  显示英文日志行（见 `AGENTS.md` 的 `CLI` 一节）。

---

## 8. 验收与测试

* **静态检查（评审）**：提交前端改动前对所有视图文件跑 `node --check`，并对 `menu.d`、`acl.d` 做 JSON 校验。
* **翻译覆盖检查**：视图中每个 `_('…')` 字符串都必须在各 `po/*` 词条表中有对应 `msgid`
  （当前 `zh_Hans` 词条已全覆盖；菜单标题由 `menu.d` 提取）。
* **消息键一致性检查**：后端发出的每个 key（`Message::new("…")`）必须在 `common.js` 的
  `MESSAGES` 表中存在，反之亦然（评审时比对）。
* **ACL 走查**：`ubus -v list wifisync` 必须与 `common.js` 中的声明一致，且每个方法在 ACL 文件
  中落在正确的读/写分组。
* **人工/浏览器回归**：`admin/services/wifisync` 下 8 个页面均可打开；非 AP 角色的 dry-run 为空；
  准入批准/拒绝流程可用；备份校验/恢复可用；非纯 AP 组合下网桥页隐藏或为空。
* **打包验证**：`release.yml` 用官方 SDK 为 OpenWrt 24.10（`.ipk`）与 25.12（`.apk`）构建
  `luci-app-wifisync`，验证 feed 包结构。

---

## 9. 前端特有风险

| 风险 | 对策 |
|------|------|
| 用户误以为 Gateway/Controller 会自动配置网络 | 常驻零侵入文案 + 空 dry-run 作为证据 + 显式确认徽标 |
| 风险写入导致设备失联 | dry-run 预览 + apply-guard 倒计时 + 一键 revert，以及各处的二次确认弹窗 |
| 只置灰不给理由会让用户困惑 | 禁用控件旁**必须**渲染禁用原因 |
| 前后端方法漂移 | 所有 `rpc.declare()` 收敛在 `common.js`；ACL 文件与 rpcd 方法表一起评审 |
| 诊断页显示的日志行只有英文 | 可接受的取舍：CLI 与服务输出必须保持纯英文（见 `AGENTS.md`），界面则提供翻译后的状态字段 |
