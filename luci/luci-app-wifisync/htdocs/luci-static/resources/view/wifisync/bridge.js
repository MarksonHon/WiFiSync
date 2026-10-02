'use strict';
'require view';
'require ui';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return Promise.all([ ws.callBridgePreview(), ws.callPlan() ]);
	},

	render: function(data) {
		var preview = data[0] || {}, plan = data[1] || {};
		var enabled = !!preview.enabled;

		var bridgeRows = (preview.bridges || []).map(function(bridge) {
			return E('tr', {}, [
				E('td', {}, bridge.name),
				E('td', {}, (bridge.ports || []).join(', ')),
				E('td', {}, bridge.vlan_filtering ? '是' : '否'),
				E('td', {}, String((bridge.vlans || []).length))
			]);
		});

		return E('div', {}, [
			enabled ? ws.notice('本设备是「纯 AP」：所有网口（含出厂 WAN 口）会被并入 br-lan。', 'warning')
				: ws.notice(preview.reason || '当前角色组合不建桥、不动网口。', 'info'),

			ws.card('网桥规划', [
				enabled && bridgeRows.length ? E('table', { 'class': 'table' }, [
					E('thead', {}, E('tr', {}, [
						E('th', {}, '网桥'), E('th', {}, '成员端口'),
						E('th', {}, 'VLAN filtering'), E('th', {}, 'VLAN 数')
					])),
					E('tbody', {}, bridgeRows)
				]) : E('div', {}, enabled ? '没有可用的物理网口。' : '无需建桥。'),
				ws.kv('网桥名', E('code', {}, preview.bridge_name || 'br-lan')),
				ws.kv('全部网口', E('span', {}, (preview.ports || []).join(', ') || '无')),
				ws.kv('LAN 口', E('span', {}, (preview.lan_ports || []).join(', ') || '无')),
				ws.kv('WAN 口', E('span', {}, (preview.wan_ports || []).join(', ') || '无'))
			], '默认只生成一个 br-lan；模型是多网桥 + VLAN 列表结构，便于后续扩展（VLAN 跨设备同步尚未启用）。'),

			ws.card('将写入的内容（dry-run）', [
				E('pre', { 'style': 'white-space:pre-wrap' }, plan.text || '（无）')
			], plan.empty ? '这是零写入计划：本角色组合不会触碰任何网络配置。' : '以上改动会在应用前自动保存快照，可随时回滚。')
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
