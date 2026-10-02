'use strict';
'require view';
'require ui';
'require poll';
'require view.wifisync.common as ws';

// Role keys reported by the backend, mapped to translatable labels.
var ROLE_LABELS = {
	controller: _('Controller'),
	ap: _('AP'),
	gateway: _('Gateway')
};

return view.extend({
	load: function() {
		return Promise.all([ ws.callStatus(), ws.callVersion() ]);
	},

	render: function(data) {
		var status = data[0] || {}, version = data[1] || {};

		var planText = E('pre', { 'class': 'cbi-input-textarea', 'style': 'white-space:pre-wrap' },
			_('(click "Preview changes" below to generate)'));

		function refreshPlan() {
			return ws.callPlan().then(function(plan) {
				planText.textContent = ws.planText(plan) || _('(no changes)');
				return plan;
			});
		}

		function applyPlan() {
			return ws.callApply().then(function(result) {
				ui.addNotification(null, planText, 'info');
				planText.textContent = '';
				alert(ws.message(result.message) || _('Applied'));
				return refreshPlan();
			}).catch(function(err) {
				alert(ws.errorText(err));
			});
		}

		var roleChips = (status.role_labels || []).map(function(label) {
			return ws.roleChip(ROLE_LABELS[label] || label);
		});

		var capabilities = status.capabilities || {};
		var failsafe = status.failsafe || {};
		var admission = status.admission || {};
		var backup = status.backup || {};

		poll.add(function() {
			return ws.callStatus().then(function(next) {
				var node = document.getElementById('wifisync-failsafe-state');
				if (node && next.failsafe)
					node.textContent = ws.message(next.failsafe.state_label);
			});
		}, 10);

		return E('div', {}, [
			ws.card(_('Roles and duties'), [
				ws.kv(_('Current roles'), E('div', {}, roleChips)),
				ws.kv(_('Automatic bridging'), status.bridge_enabled
					? E('span', { 'class': 'label success' }, _('Allowed (pure AP role)'))
					: E('span', { 'class': 'label' }, ws.message(status.bridge_blocked_reason) || _('Not allowed'))),
				ws.kv(_('Device ID'), E('code', {}, status.device_id || '-')),
				ws.kv(_('Sync mode'), E('span', {}, status.sync_mode || 'auto')),
				ws.kv(_('Version'), E('span', {}, (version.version || '-') + ' (protocol ' + (version.protocol || 0) + ')'))
			], _('Gateway / Controller are read-only, probe-only and push-only; only a "pure AP" device (AP without Controller and Gateway) merges all ports into br-lan.')),

			ws.card(_('Hardware capabilities'), [
				ws.kv(_('Device model'), E('span', {}, capabilities.model || capabilities.board_name || _('unknown'))),
				ws.kv(_('Wireless radios'), E('span', {}, (capabilities.radios || []).map(function(r) { return r.name; }).join(', ') || _('none'))),
				ws.kv(_('Full wpad (KVR)'), ws.boolLabel(capabilities.wpad_full)),
				ws.kv(_('VLAN capable'), ws.boolLabel(capabilities.vlan_capable)),
				ws.kv(_('Ports'), E('span', {}, (capabilities.ports || []).map(function(p) {
					return p.name + '(' + p.kind + ')';
				}).join(', ') || _('none')))
			]),

			ws.card(_('Failover (optional)'), [
				ws.kv(_('State'), E('span', { 'id': 'wifisync-failsafe-state' }, ws.message(failsafe.state_label) || '-')),
				ws.kv(_('Applicable'), ws.boolLabel(failsafe.applies)),
				ws.kv(_('Heartbeat target'), E('span', {}, (failsafe.config && failsafe.config.heartbeat_endpoint) || _('not configured')))
			], _('This feature is off by default. Only AP devices restore the default network when the Controller/Gateway becomes unreachable.')),

			ws.card(_('Admission and backup'), [
				ws.kv(_('Pending APs'), E('span', {}, String(admission.pending || 0))),
				ws.kv(_('Approved'), E('span', {}, String(admission.approved || 0))),
				ws.kv(_('Initial baseline'), backup.initial_exists
					? E('span', { 'class': 'label success' }, _('established'))
					: E('span', { 'class': 'label warning' }, _('not established'))),
				ws.kv(_('Dirty shutdown'), backup.dirty
					? E('span', { 'class': 'label warning' }, ws.message(backup.dirty_reason) || _('Yes'))
					: ws.boolLabel(false))
			]),

			ws.card(_('Change preview and apply'), [
				E('div', { 'class': 'cbi-value' }, [
					ws.submit(_('Preview changes (dry-run)'), function() { refreshPlan(); }),
					' ',
					ws.submit(_('Apply changes'), applyPlan),
					' ',
					ws.submit(_('Confirm apply'), function() {
						ws.callConfirm().then(function() { alert(_('Confirmed, apply-guard released')); });
					}),
					' ',
					ws.submit(_('Revert last write'), function() {
						if (!confirm(_('Revert the most recent write?'))) return;
						ws.callRevert().then(function(r) { alert(ws.message(r.report) || _('Reverted')); });
					})
				]),
				planText
			], _('For non-AP roles the preview is always "0 changes" — the proof of the zero-intrusion design.'))
		].filter(Boolean));
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
