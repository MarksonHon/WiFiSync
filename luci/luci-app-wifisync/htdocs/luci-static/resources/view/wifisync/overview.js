'use strict';
'require view';
'require ui';
'require poll';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return Promise.all([ ws.callStatus(), ws.callVersion() ]);
	},

	render: function(data) {
		var status = data[0] || {}, version = data[1] || {};
		var self = this;

		var planText = E('pre', { 'class': 'cbi-input-textarea', 'style': 'white-space:pre-wrap' }, '（点击下方"预览改动"生成）');

		function refreshPlan() {
			return ws.callPlan().then(function(plan) {
				planText.textContent = plan.text + '\n\n' + (plan.notes || []).join('\n');
				return plan;
			});
		}

		function applyPlan() {
			return ws.callApply().then(function(result) {
				ui.addNotification(null, planText, 'info');
				planText.textContent = '';
				alert(result.message || '已应用');
				return refreshPlan();
			}).catch(function(err) {
				alert(err.message || err);
			});
		}

		var roleChips = (status.role_labels || []).map(function(label) {
			return ws.roleChip(label);
		});

		var capabilities = status.capabilities || {};
		var failsafe = status.failsafe || {};
		var admission = status.admission || {};
		var backup = status.backup || {};

		poll.add(function() {
			return ws.callStatus().then(function(next) {
				var node = document.getElementById('wifisync-failsafe-state');
				if (node && next.failsafe)
					node.textContent = next.failsafe.state_label;
			});
		}, 10);

		return E('div', {}, [
			ws.card('角色与职责', [
				ws.kv('当前角色', E('div', {}, roleChips)),
				ws.kv('是否自动建桥', status.bridge_enabled
					? E('span', { 'class': 'label success' }, '允许（纯 AP 角色）')
					: E('span', { 'class': 'label' }, status.bridge_blocked_reason || '不允许')),
				ws.kv('设备 ID', E('code', {}, status.device_id || '-')),
				ws.kv('同步模式', E('span', {}, status.sync_mode || 'auto')),
				ws.kv('版本', E('span', {}, (version.version || '-') + ' (protocol ' + (version.protocol || 0) + ')'))
			], 'Gateway / Controller 只读、只探测、只下发；只有「纯 AP」设备（有 AP 且无 Controller、无 Gateway）才会把所有网口并入 br-lan。'),

			ws.card('硬件能力', [
				ws.kv('设备型号', E('span', {}, capabilities.model || capabilities.board_name || '未知')),
				ws.kv('无线 radio', E('span', {}, (capabilities.radios || []).map(function(r) { return r.name; }).join(', ') || '无')),
				ws.kv('完整版 wpad（KVR）', ws.boolLabel(capabilities.wpad_full)),
				ws.kv('VLAN 能力', ws.boolLabel(capabilities.vlan_capable)),
				ws.kv('网口', E('span', {}, (capabilities.ports || []).map(function(p) {
					return p.name + '(' + p.kind + ')';
				}).join(', ') || '无'))
			]),

			ws.card('故障恢复（可选）', [
				ws.kv('状态', E('span', { 'id': 'wifisync-failsafe-state' }, failsafe.state_label || '-')),
				ws.kv('适用性', ws.boolLabel(failsafe.applies)),
				ws.kv('心跳目标', E('span', {}, (failsafe.config && failsafe.config.heartbeat_endpoint) || '未配置'))
			], '该功能默认关闭。仅 AP 设备会在连不上 Controller/Gateway 时恢复设备默认网络。'),

			ws.card('准入与备份', [
				ws.kv('待批准 AP', E('span', {}, String(admission.pending || 0))),
				ws.kv('已批准', E('span', {}, String(admission.approved || 0))),
				ws.kv('初始化基线', backup.initial_exists
					? E('span', { 'class': 'label success' }, '已建立')
					: E('span', { 'class': 'label warning' }, '未建立')),
				ws.kv('上次退出是否异常', backup.dirty
					? E('span', { 'class': 'label warning' }, backup.dirty_reason || '是')
					: ws.boolLabel(false))
			]),

			ws.card('改动预览与应用', [
				E('div', { 'class': 'cbi-value' }, [
					ws.submit('预览改动（dry-run）', function() { refreshPlan(); }),
					' ',
					ws.submit('应用改动', applyPlan),
					' ',
					ws.submit('确认应用成功', function() {
						ws.callConfirm().then(function() { alert('已确认，apply-guard 已解除'); });
					}),
					' ',
					ws.submit('回滚最近一次写入', function() {
						if (!confirm('确定回滚最近一次写入吗？')) return;
						ws.callRevert().then(function(r) { alert(r.report || '已回滚'); });
					})
				]),
				planText
			], '非 AP 角色下预览结果恒为「0 项改动」—— 这是"零侵入"的证明。')
		].filter(Boolean));
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
