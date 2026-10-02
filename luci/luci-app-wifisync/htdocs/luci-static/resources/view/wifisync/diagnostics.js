'use strict';
'require view';
'require ui';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return Promise.all([ ws.callLogs(200), ws.callPlan(), ws.callStatus(), ws.callVersion() ]);
	},

	render: function(data) {
		var logs = data[0] || {}, plan = data[1] || {}, status = data[2] || {}, version = data[3] || {};
		var logBox = E('pre', { 'style': 'white-space:pre-wrap; max-height:24em; overflow:auto' },
			(logs.lines || []).join('\n') || '（暂无日志：服务的 stderr 由 procd 收集，可用 logread 查看）');

		function refreshLogs() {
			return ws.callLogs(200).then(function(next) {
				logBox.textContent = (next.lines || []).join('\n') || '（暂无日志）';
			});
		}

		function copy(el) {
			if (navigator.clipboard) {
				navigator.clipboard.writeText(el.textContent);
				ui.addNotification(null, E('p', {}, '已复制到剪贴板'), 'info');
			}
		}

		var diagnostics = JSON.stringify({
			version: version,
			status: status,
			plan: plan
		}, null, 2);

		var diagBox = E('pre', { 'style': 'white-space:pre-wrap; max-height:24em; overflow:auto' }, diagnostics);

		return E('div', {}, [
			ws.card('自检信息（可直接贴到 issue）', [
				diagBox,
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit('复制', function() { copy(diagBox); })
					])
				])
			]),

			ws.card('写入计划（dry-run 全量）', [
				E('pre', { 'style': 'white-space:pre-wrap' }, plan.text || '（无）'),
				E('div', { 'class': 'cbi-section-descr' },
					'非 AP 角色时此处恒为「0 项改动」，这是零侵入设计的直接证据。')
			]),

			ws.card('服务日志', [
				logBox,
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit('刷新日志', refreshLogs),
						' ',
						ws.submit('复制日志', function() { copy(logBox); })
					])
				])
			], '服务日志同时写入 syslog，可用 `logread -e wifisync` 查看。'),

			ws.card('常用诊断命令', [
				E('pre', { 'style': 'white-space:pre-wrap' },
					'/usr/bin/wifisync status\n' +
					'/usr/bin/wifisync plan\n' +
					'/usr/bin/wifisync backup list\n' +
					'/usr/bin/wifisync backup verify\n' +
					'/usr/bin/wifisync probe 192.168.1.1\n' +
					'ubus call wifisync status\n' +
					'logread -e wifisync')
			])
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
