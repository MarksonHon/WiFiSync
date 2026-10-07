'use strict';
'require view';
'require ui';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return Promise.all([ ws.callWifiSourceGet(), ws.callCapabilities() ]);
	},

	render: function(data) {
		var source = data[0] || {}, caps = data[1] || {};
		var kvrAvailable = !!(source.kvr && source.kvr.available);
		var plan = E('pre', { 'style': 'white-space:pre-wrap' }, _('(refreshed after saving)'));
		var message = E('div', {});

		var endpoints = [];

		function renderKvrNote() {
			if (kvrAvailable) return ws.notice(_('Full wpad detected: 802.11k/v/r is available.'), 'success');
			return ws.notice(_('KVR unavailable') + ': ' + (ws.message(source.kvr && source.kvr.note) || _('the full wpad package is required')) +
				'. ' + _('Install wpad-mbedtls (or wpad) instead of wpad-basic-mbedtls.'), 'error');
		}

		function renderPlan() {
			return ws.callPlan().then(function(next) {
				plan.textContent = ws.planText(next) || _('(none)');
				return next;
			});
		}
		renderPlan();

		var domain = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': '' });
		var flags = {};
		[ 'ieee80211k', 'ieee80211v', 'ieee80211r', 'ft_over_ds' ].forEach(function(name) {
			flags[name] = E('input', { 'type': 'checkbox', 'checked': '' });
			endpoints.push(E('div', { 'class': 'cbi-value' }, [
				E('label', { 'class': 'cbi-value-title' }, E('span', {}, [ flags[name], ' ', name ])),
				E('div', { 'class': 'cbi-value-field' }, kvrAvailable ? _('Enabled') : _('Ignored when wpad does not support it'))
			]));
		});

		function save() {
			if (!kvrAvailable) {
				message.innerHTML = '';
				message.appendChild(ws.notice(_('Refusing to enable KVR: the full wpad package is missing.'), 'error'));
				return;
			}
			var custom = {
				// Blank SSID and mobility domain: the backend derives them from the SSID.
				ssid: '', auth: 'sae-mixed', psk_ref: 'custom', band: '5g',
				mobility_domain: domain.value.trim(),
				ieee80211k: flags.ieee80211k.checked,
				ieee80211v: flags.ieee80211v.checked,
				ieee80211r: flags.ieee80211r.checked,
				ft_over_ds: flags.ft_over_ds.checked
			};
			return ws.callWifiSourceSet('custom', '', false, custom).then(function() {
				return renderPlan();
			}).then(function() {
				message.innerHTML = '';
				message.appendChild(ws.notice(_('KVR parameters saved (distributed to APs with the NetworkProfile)'), 'success'));
			}).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(ws.errorText(err), 'error'));
			});
		}

		// Wi-Fi relay notes (no mesh support)
		return E('div', {}, [
			ws.card(_('KVR (802.11k / v / r)'), [
				renderKvrNote(),
				ws.kv(_('Device radios'), E('span', {}, (caps.radios || []).map(function(r) {
					return r.name + (r.band ? '(' + r.band + ')' : '');
				}).join(', ') || _('none'))),
				E('div', { 'class': 'cbi-section-descr' },
					_('Leave blank to derive it from the SSID (4 hex digits otherwise)')),
				ws.kv(_('Mobility domain'), domain),
			].concat(endpoints).concat([
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [ ws.submit(_('Save KVR parameters'), save) ])
				]),
				message
			]), _('All KVR parameters are distributed to every AP with the NetworkProfile pushed by the Controller, keeping mobility_domain consistent.')),

			ws.card(_('Wi-Fi relay (no mesh)'), [
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' },
						_('Relay always uses a standard Wi-Fi relay: wpa_supplicant(STA) upstream plus hostapd(AP) downstream.') +
						_('This program does not implement mesh networking such as 802.11s or batman.') +
						_('If 802.11r is restricted on the relay link, it automatically falls back to ft_over_ds.'))
				])
			]),

			ws.card(_('Resulting write plan'), [ plan ])
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
