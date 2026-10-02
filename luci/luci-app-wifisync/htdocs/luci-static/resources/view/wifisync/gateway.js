'use strict';
'require view';
'require ui';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return Promise.all([ ws.callStatus(), ws.callCapabilities() ]);
	},

	render: function(data) {
		var status = data[0] || {}, caps = data[1] || {};
		var ports = caps.ports || [];
		var selected = {};
		(status.gateway_lan_ifaces || []).forEach(function(name) { selected[name] = true; });

		var boxes = {};
		var rows = ports.map(function(port) {
			var input = E('input', {
				'type': 'checkbox',
				'checked': selected[port.name] ? '' : null,
				'change': function(ev) { selected[port.name] = ev.target.checked; }
			});
			boxes[port.name] = input;
			return E('div', { 'class': 'cbi-value' }, [
				E('label', { 'class': 'cbi-value-title' }, E('span', {}, [ input, ' ', port.name ])),
				E('div', { 'class': 'cbi-value-field' },
					_('Factory role') + ': ' + port.kind + '   DSA: ' + (port.dsa ? _('Yes') : _('No')) +
					'   ' + _('Carrier') + ': ' + (port.carrier ? _('Yes') : _('No')))
			]);
		});

		var message = E('div', {});
		var endpoint = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'style': 'width:20em',
			'placeholder': _('e.g. 192.168.1.1 or 192.168.1.1:443') });
		var probeResult = E('div', {});

		function save() {
			var interfaces = Object.keys(selected).filter(function(key) { return selected[key]; });
			return ws.callGatewaySet(interfaces).then(function(result) {
				message.innerHTML = '';
				message.appendChild(ws.notice(ws.message(result.message) || _('Saved'), 'success'));
				if (result.network_modified)
					message.appendChild(ws.notice(_('Warning: the local write plan is not empty, please check the overview page!'), 'error'));
			}).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(ws.errorText(err), 'error'));
			});
		}

		function probe() {
			var target = endpoint.value.trim();
			if (!target) return;
			ws.callProbe(target).then(function(result) {
				probeResult.innerHTML = '';
				probeResult.appendChild(ws.notice(
					target + ': ' + (result.reachable ? _('reachable') : _('unreachable')) +
					' (' + _('method') + ' ' + result.method + ', ' +
					_('latency') + ' ' + (result.latency_ms !== null ? result.latency_ms + 'ms' : '-') + ')' +
					'  ' + ws.message(result.detail),
					result.reachable ? 'success' : 'warning'));
			});
		}

		return E('div', {}, [
			ws.notice(_('The Gateway role never changes your network for you: here you only select the "corresponding LAN interface", which is used for topology identification and connectivity probing. The program does not modify routes, NAT, the firewall or any content of /etc/config/network.'), 'info'),

			ws.card(_('Corresponding LAN interfaces'), rows.length ? rows : [
				ws.notice(_('No physical ports were detected (possibly a host or virtual environment).'), 'warning')
			], _('After selecting them, open the overview page and run a dry-run to confirm that it reports 0 changes.')),

			ws.card(_('Connectivity probe (read-only)'), [
				E('div', { 'class': 'cbi-value' }, [
					E('label', { 'class': 'cbi-value-title' }, _('Probe target')),
					E('div', { 'class': 'cbi-value-field' }, [ endpoint, ' ', ws.submit(_('Probe'), probe) ])
				]),
				probeResult,
				E('div', { 'class': 'cbi-section-descr' },
					_('Whether the Controller can reach the Gateway is confirmed by you separately: the program only performs ICMP/TCP probes and never writes the routing table, changes the firewall or touches NAT.'))
			]),

			ws.card(_('Save'), [
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [ ws.submit(_('Save LAN interface selection'), save) ])
				]),
				message
			])
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
