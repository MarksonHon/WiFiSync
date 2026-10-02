'use strict';
'require view';
'require ui';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return Promise.all([ ws.callBackupList(), ws.callFailsafeGet(), ws.callStatus() ]);
	},

	render: function(data) {
		var backups = data[0] || {}, failsafe = data[1] || {}, status = data[2] || {};
		var message = E('div', {});
		var verifyResult = E('div', {});

		function notify(text, type) {
			message.innerHTML = '';
			message.appendChild(ws.notice(text, type || 'success'));
		}

		function refresh() {
			return ws.callBackupList().then(function(next) {
				backups = next;
				tableBody.innerHTML = '';
				renderRows();
			});
		}

		var tableBody = E('tbody', {});
		function renderRows() {
			var items = backups.snapshots || [];
			if (!items.length) {
				tableBody.appendChild(E('tr', {}, [
					E('td', { 'colspan': 5, 'style': 'text-align:center' }, '暂无快照')
				]));
				return;
			}
			items.slice().reverse().forEach(function(snapshot) {
				var when = new Date(snapshot.created_at * 1000).toISOString().replace('T', ' ').replace('.000Z', ' UTC');
				tableBody.appendChild(E('tr', {}, [
					E('td', {}, snapshot.kind),
					E('td', {}, when),
					E('td', {}, Math.round((snapshot.bytes || 0) / 1024) + ' KiB'),
					E('td', {}, snapshot.path),
					E('td', {}, [
						ws.submit('校验', function() {
							ws.callBackupVerify(snapshot.path).then(function(report) {
								verifyResult.innerHTML = '';
								verifyResult.appendChild(ws.notice(report.summary, report.ok ? 'success' : 'error'));
							});
						})
					])
				]));
			});
		}
		renderRows();

		function doRestore(mode) {
			var hint = mode === 'full'
				? '将按基线整体覆盖 network/wireless/dhcp/firewall/system，用户在这些文件里的改动都会丢失。确定继续？'
				: '只会还原 WifiSync 改动过的键，其它配置保持不变。确定继续？';
			if (!confirm(hint)) return;
			ws.callRestore(mode, 'initial').then(function(result) {
				notify('恢复完成：' + (result.report || '') + '（' + mode + '）');
				return refresh();
			}).catch(function(err) {
				notify(String(err.message || err), 'error');
			});
		}

		// 故障恢复表单
		var enabled = E('input', { 'type': 'checkbox', 'checked': (failsafe.config && failsafe.config.enabled) ? '' : null });
		var timeout = E('input', { 'type': 'text', 'class': 'cbi-input-text',
			'value': String((failsafe.config && failsafe.config.link_timeout_secs) || 300) });
		var confirmSecs = E('input', { 'type': 'text', 'class': 'cbi-input-text',
			'value': String((failsafe.config && failsafe.config.apply_confirm_secs) || 90) });
		var action = E('select', { 'class': 'cbi-input-select' }, [
			E('option', { 'value': 'revert', 'selected': ((failsafe.config && failsafe.config.action) === 'revert') ? '' : null }, '恢复默认网络'),
			E('option', { 'value': 'reboot', 'selected': ((failsafe.config && failsafe.config.action) === 'reboot') ? '' : null }, '恢复后重启')
		]);
		var heartbeat = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'style': 'width:20em',
			'value': (failsafe.config && failsafe.config.heartbeat_endpoint) || '' });

		function saveFailsafe() {
			ws.callFailsafeSet(
				enabled.checked,
				parseInt(confirmSecs.value, 10) || 90,
				parseInt(timeout.value, 10) || 300,
				action.value,
				false,
				heartbeat.value.trim()
			).then(function() {
				notify('故障恢复设置已保存');
			}).catch(function(err) {
				notify(String(err.message || err), 'error');
			});
		}

		return E('div', {}, [
			ws.notice('纪律：服务启动前自动保存初始化网络基线，服务停止前自动恢复原有网络。' +
				'恢复默认范围是 managed_only（只还原 WifiSync 改过的键）。', 'info'),

			ws.card('初始化基线', [
				ws.kv('基线状态', backups.initial_exists
					? E('span', { 'class': 'label success' }, '已建立')
					: E('span', { 'class': 'label warning' }, '未建立（首次启动服务时创建）')),
				ws.kv('基线路径', E('code', {}, backups.initial_dir || '-')),
				ws.kv('上次退出', (status.backup && status.backup.dirty)
					? E('span', { 'class': 'label warning' }, status.backup.dirty_reason || '异常')
					: E('span', { 'class': 'label success' }, '正常')),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit('校验基线完整性', function() {
							ws.callBackupVerify('').then(function(report) {
								verifyResult.innerHTML = '';
								verifyResult.appendChild(ws.notice(report.summary, report.ok ? 'success' : 'error'));
							});
						}), ' ',
						ws.submit('立即创建快照', function() {
							ws.callBackupCreate('pre-change').then(function() {
								notify('快照已创建');
								return refresh();
							});
						}), ' ',
						ws.submit('清理旧快照', function() {
							ws.callBackupPrune().then(function(result) {
								notify('已清理 ' + ((result.removed || []).length) + ' 份旧快照（基线永不清理）');
								return refresh();
							});
						})
					])
				]),
				verifyResult,
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit('恢复基线（managed_only）', function() { doRestore('managed_only'); }), ' ',
						ws.submit('恢复基线（full）', function() { doRestore('full'); })
					])
				]),
				message
			], '恢复前会先校验 sha256；校验不通过会直接拒绝执行，且永远不会删除基线。'),

			ws.card('快照列表', [
				E('table', { 'class': 'table' }, [
					E('thead', {}, E('tr', {}, [
						E('th', {}, '类型'), E('th', {}, '时间'), E('th', {}, '大小'),
						E('th', {}, '路径'), E('th', {}, '操作')
					])),
					tableBody
				])
			]),

			ws.card('故障恢复（可选，默认关闭）', [
				E('div', { 'class': 'cbi-value' }, [
					E('label', { 'class': 'cbi-value-title' }, E('span', {}, [ enabled, ' 启用' ])),
					E('div', { 'class': 'cbi-value-field' },
						'仅 AP 设备生效：连不上 Controller/Gateway 超过设定时间后恢复设备默认网络。' +
						'Gateway / Controller 不会触发这类回滚（零侵入原则）。')
				]),
				ws.kv('心跳丢失超时（秒）', timeout),
				ws.kv('应用确认窗口（秒）', confirmSecs),
				ws.kv('触发动作', action),
				ws.kv('心跳目标（Controller/Gateway）', heartbeat),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [ ws.submit('保存故障恢复设置', saveFailsafe) ])
				])
			])
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
