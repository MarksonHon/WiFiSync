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
		var plan = E('pre', { 'style': 'white-space:pre-wrap' }, '（保存后自动刷新）');
		var message = E('div', {});

		var endpoints = [];

		function renderKvrNote() {
			if (kvrAvailable) return ws.notice('检测到完整版 wpad，802.11k/v/r 可用。', 'success');
			return ws.notice('KVR 不可用：' + ((source.kvr && source.kvr.note) || '需要完整版 wpad') +
				'。请安装 `wpad-mbedtls`（或 `wpad`）而不是 `wpad-basic-mbedtls`。', 'error');
		}

		function renderPlan() {
			return ws.callPlan().then(function(next) {
				plan.textContent = next.text || '（无）';
				return next;
			});
		}
		renderPlan();

		var domain = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': '1a2b' });
		var flags = {};
		[ 'ieee80211k', 'ieee80211v', 'ieee80211r', 'ft_over_ds' ].forEach(function(name) {
			flags[name] = E('input', { 'type': 'checkbox', 'checked': '' });
			endpoints.push(E('div', { 'class': 'cbi-value' }, [
				E('label', { 'class': 'cbi-value-title' }, E('span', {}, [ flags[name], ' ', name ])),
				E('div', { 'class': 'cbi-value-field' }, kvrAvailable ? '启用' : 'wpad 不支持时会被忽略')
			]));
		});

		function save() {
			if (!kvrAvailable) {
				message.innerHTML = '';
				message.appendChild(ws.notice('缺少完整版 wpad，拒绝开启 KVR。', 'error'));
				return;
			}
			var custom = {
				ssid: 'WifiSync', auth: 'sae-mixed', psk_ref: 'custom', band: '5g',
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
				message.appendChild(ws.notice('KVR 参数已保存（随 NetworkProfile 下发给 AP）', 'success'));
			}).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(String(err.message || err), 'error'));
			});
		}

		// Wi-Fi 中继说明（不实现 mesh）
		return E('div', {}, [
			ws.card('KVR（802.11k / v / r）', [
				renderKvrNote(),
				ws.kv('设备 radio', E('span', {}, (caps.radios || []).map(function(r) {
					return r.name + (r.band ? '(' + r.band + ')' : '');
				}).join(', ') || '无')),
				ws.kv('漫游域 mobility_domain', domain),
			].concat(endpoints).concat([
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [ ws.submit('保存 KVR 参数', save) ])
				]),
				message
			]), '所有 KVR 参数随 Controller 下发的 NetworkProfile 统一下发到各 AP，保证 mobility_domain 一致。'),

			ws.card('Wi-Fi 中继（不使用 mesh）', [
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' },
						'中继一律使用标准 Wi-Fi 中继：上行 wpa_supplicant(STA) + 下行 hostapd(AP)。' +
						'本程序不实现 802.11s / batman 等 mesh 组网。' +
						'中继链路下若 802.11r 受限，会自动回退到 ft_over_ds。')
				])
			]),

			ws.card('对应的写入计划', [ plan ])
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
