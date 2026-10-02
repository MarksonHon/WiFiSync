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
					E('td', { 'colspan': 5, 'style': 'text-align:center' }, _('No snapshots'))
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
						ws.submit(_('Verify'), function() {
							ws.callBackupVerify(snapshot.path).then(function(report) {
								verifyResult.innerHTML = '';
								verifyResult.appendChild(ws.notice(ws.message(report.summary), report.ok ? 'success' : 'error'));
							});
						})
					])
				]));
			});
		}
		renderRows();

		function doRestore(mode) {
			var hint = mode === 'full'
				? _('network/wireless/dhcp/firewall/system will be overwritten from the baseline as a whole; user changes in these files will be lost. Continue?')
				: _('Only the keys changed by WifiSync are restored, all other configuration is preserved. Continue?');
			if (!confirm(hint)) return;
			ws.callRestore(mode, 'initial').then(function(result) {
				notify(_('Restore finished') + ': ' + ws.message(result.report) + ' (' + mode + ')');
				return refresh();
			}).catch(function(err) {
				notify(ws.errorText(err), 'error');
			});
		}

		// Failover form
		var enabled = E('input', { 'type': 'checkbox', 'checked': (failsafe.config && failsafe.config.enabled) ? '' : null });
		var timeout = E('input', { 'type': 'text', 'class': 'cbi-input-text',
			'value': String((failsafe.config && failsafe.config.link_timeout_secs) || 300) });
		var confirmSecs = E('input', { 'type': 'text', 'class': 'cbi-input-text',
			'value': String((failsafe.config && failsafe.config.apply_confirm_secs) || 90) });
		var action = E('select', { 'class': 'cbi-input-select' }, [
			E('option', { 'value': 'revert', 'selected': ((failsafe.config && failsafe.config.action) === 'revert') ? '' : null }, _('Restore the default network')),
			E('option', { 'value': 'reboot', 'selected': ((failsafe.config && failsafe.config.action) === 'reboot') ? '' : null }, _('Restore and reboot'))
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
				notify(_('Failover settings saved'));
			}).catch(function(err) {
				notify(ws.errorText(err), 'error');
			});
		}

		return E('div', {}, [
			ws.notice(_('Discipline: the initial network baseline is saved before the service starts and the previous network is restored before it stops.') +
				_('The default restore scope is managed_only (only the keys changed by WifiSync are restored).'), 'info'),

			ws.card(_('Initial baseline'), [
				ws.kv(_('Baseline state'), backups.initial_exists
					? E('span', { 'class': 'label success' }, _('established'))
					: E('span', { 'class': 'label warning' }, _('not established (created on the first service start)'))),
				ws.kv(_('Baseline path'), E('code', {}, backups.initial_dir || '-')),
				ws.kv(_('Last shutdown'), (status.backup && status.backup.dirty)
					? E('span', { 'class': 'label warning' }, ws.message(status.backup.dirty_reason) || _('abnormal'))
					: E('span', { 'class': 'label success' }, _('normal'))),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit(_('Verify baseline integrity'), function() {
							ws.callBackupVerify('').then(function(report) {
								verifyResult.innerHTML = '';
								verifyResult.appendChild(ws.notice(ws.message(report.summary), report.ok ? 'success' : 'error'));
							});
						}), ' ',
						ws.submit(_('Create snapshot now'), function() {
							ws.callBackupCreate('pre-change').then(function() {
								notify(_('Snapshot created'));
								return refresh();
							});
						}), ' ',
						ws.submit(_('Prune old snapshots'), function() {
							ws.callBackupPrune().then(function(result) {
								notify(_('Pruned old snapshots') + ': ' + ((result.removed || []).length) + ' ' + _('(the baseline is never pruned)'));
								return refresh();
							});
						})
					])
				]),
				verifyResult,
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit(_('Restore baseline (managed_only)'), function() { doRestore('managed_only'); }), ' ',
						ws.submit(_('Restore baseline (full)'), function() { doRestore('full'); })
					])
				]),
				message
			], _('The sha256 checksum is verified before restoring; a failed check rejects the restore and the baseline is never deleted.')),

			ws.card(_('Snapshot list'), [
				E('table', { 'class': 'table' }, [
					E('thead', {}, E('tr', {}, [
						E('th', {}, _('Kind')), E('th', {}, _('Time')), E('th', {}, _('Size')),
						E('th', {}, _('Path')), E('th', {}, _('Actions'))
					])),
					tableBody
				])
			]),

			ws.card(_('Failover (optional, off by default)'), [
				E('div', { 'class': 'cbi-value' }, [
					E('label', { 'class': 'cbi-value-title' }, E('span', {}, [ enabled, ' ', _('Enable') ])),
					E('div', { 'class': 'cbi-value-field' },
						_('Only effective on AP devices: the default network is restored after the Controller/Gateway has been unreachable for the configured time.') +
						_('Gateway / Controller never trigger such a rollback (zero-intrusion principle).'))
				]),
				ws.kv(_('Heartbeat loss timeout (seconds)'), timeout),
				ws.kv(_('Apply confirmation window (seconds)'), confirmSecs),
				ws.kv(_('Trigger action'), action),
				ws.kv(_('Heartbeat target (Controller/Gateway)'), heartbeat),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [ ws.submit(_('Save failover settings'), saveFailsafe) ])
				])
			])
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
