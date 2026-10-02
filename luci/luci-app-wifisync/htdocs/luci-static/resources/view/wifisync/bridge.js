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
				E('td', {}, bridge.vlan_filtering ? _('Yes') : _('No')),
				E('td', {}, String((bridge.vlans || []).length))
			]);
		});

		return E('div', {}, [
			enabled ? ws.notice(_('This device is a "pure AP": all ports (including the factory WAN port) are merged into br-lan.'), 'warning')
				: ws.notice(ws.message(preview.reason) || _('The current role combination creates no bridge and touches no port.'), 'info'),

			ws.card(_('Bridge plan'), [
				enabled && bridgeRows.length ? E('table', { 'class': 'table' }, [
					E('thead', {}, E('tr', {}, [
						E('th', {}, _('Bridge')), E('th', {}, _('Member ports')),
						E('th', {}, _('VLAN filtering')), E('th', {}, _('VLANs'))
					])),
					E('tbody', {}, bridgeRows)
				]) : E('div', {}, enabled ? _('No usable physical port.') : _('No bridge needed.')),
				ws.kv(_('Bridge name'), E('code', {}, preview.bridge_name || 'br-lan')),
				ws.kv(_('All ports'), E('span', {}, (preview.ports || []).join(', ') || _('none'))),
				ws.kv(_('LAN ports'), E('span', {}, (preview.lan_ports || []).join(', ') || _('none'))),
				ws.kv(_('WAN ports'), E('span', {}, (preview.wan_ports || []).join(', ') || _('none')))
			], _('By default only one br-lan is generated; the model is a multi-bridge + VLAN list structure for future extensions (cross-device VLAN sync is not enabled yet).')),

			ws.card(_('What would be written (dry-run)'), [
				E('pre', { 'style': 'white-space:pre-wrap' }, ws.planText(plan) || _('(none)'))
			], plan.empty ? _('This is a zero-write plan: the current role combination does not touch any network configuration.') : _('A snapshot is taken automatically before these changes are applied, so they can be rolled back at any time.'))
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
