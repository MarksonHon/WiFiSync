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
			(logs.lines || []).join('\n') || _('(no logs yet: the stderr of the service is collected by procd, use logread to inspect it)'));

		function refreshLogs() {
			return ws.callLogs(200).then(function(next) {
				logBox.textContent = (next.lines || []).join('\n') || _('(no logs yet)');
			});
		}

		function copy(el) {
			if (navigator.clipboard) {
				navigator.clipboard.writeText(el.textContent);
				ui.addNotification(null, E('p', {}, _('Copied to clipboard')), 'info');
			}
		}

		var diagnostics = JSON.stringify({
			version: version,
			status: status,
			plan: plan
		}, null, 2);

		var diagBox = E('pre', { 'style': 'white-space:pre-wrap; max-height:24em; overflow:auto' }, diagnostics);

		return E('div', {}, [
			ws.card(_('Self-check information (can be pasted into an issue)'), [
				diagBox,
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit(_('Copy'), function() { copy(diagBox); })
					])
				])
			]),

			ws.card(_('Write plan (full dry-run)'), [
				E('pre', { 'style': 'white-space:pre-wrap' }, ws.planText(plan) || _('(none)')),
				E('div', { 'class': 'cbi-section-descr' },
					_('For non-AP roles this is always "0 changes", which is direct evidence of the zero-intrusion design.'))
			]),

			ws.card(_('Service log'), [
				logBox,
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit(_('Refresh log'), refreshLogs),
						' ',
						ws.submit(_('Copy log'), function() { copy(logBox); })
					])
				])
			], _('The service log is also written to syslog and can be inspected with `logread -e wifisync`.')),

			ws.card(_('Common diagnostic commands'), [
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
