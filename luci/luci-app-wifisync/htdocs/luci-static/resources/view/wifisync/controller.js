'use strict';
'require view';
'require ui';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return Promise.all([
			ws.callAdmissionList(),
			ws.callWifiSourceGet(),
			ws.callStatus()
		]);
	},

	render: function(data) {
		var admission = data[0] || {}, source = data[1] || {}, status = data[2] || {};
		var isController = !!(status.roles && status.roles.controller);

		var message = E('div', {});

		function refresh() {
			return ws.callAdmissionList().then(function(next) {
				admission = next;
				renderTable();
				return next;
			});
		}

		function setState(method, deviceId, label) {
			if (!confirm(label + ' 设备 ' + deviceId + ' ？')) return;
			method(deviceId).then(refresh).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(String(err.message || err), 'error'));
			});
		}

		var tableBody = E('tbody', {});
		function renderTable() {
			tableBody.innerHTML = '';
			var entries = admission.entries || [];
			if (!entries.length) {
				tableBody.appendChild(E('tr', {}, [
					E('td', { 'colspan': 6, 'style': 'text-align:center' }, '暂无设备上报')
				]));
				return;
			}
			entries.forEach(function(entry) {
				var stateLabel = {
					pending: E('span', { 'class': 'label warning' }, '待批准'),
					approved: E('span', { 'class': 'label success' }, '已批准'),
					rejected: E('span', { 'class': 'label' }, '已拒绝')
				}[entry.state] || entry.state;

				tableBody.appendChild(E('tr', {}, [
					E('td', {}, entry.device_id.slice(0, 8)),
					E('td', {}, entry.mac || '-'),
					E('td', {}, entry.hostname || '-'),
					E('td', {}, entry.source_addr || '-'),
					E('td', {}, stateLabel),
					E('td', {}, [
						ws.submit('批准', function() { setState(ws.callAdmissionApprove, entry.device_id, '批准'); }),
						' ',
						ws.submit('拒绝', function() { setState(ws.callAdmissionReject, entry.device_id, '拒绝'); }),
						' ',
						ws.submit('撤销', function() { setState(ws.callAdmissionRevoke, entry.device_id, '撤销批准'); })
					])
				]));
			});
		}
		renderTable();

		// ── Wi-Fi 信息源 ────────────────────────────────────────────────────
		var availability = {};
		(source.availability || []).forEach(function(item) { availability[item.kind] = item; });
		var currentKind = (source.config && source.config.kind) || 'controller_self';
		var endpoint = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'style': 'width:20em',
			'value': (source.config && source.config.gateway_endpoint) || '' });

		var custom = (source.config && source.config.custom) || {};
		var ssid = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': custom.ssid || 'WifiSync' });
		var auth = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': custom.auth || 'sae-mixed' });
		var pskRef = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': custom.psk_ref || 'custom' });
		var band = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': custom.band || '5g' });
		var domain = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': (custom.kvr && custom.kvr.mobility_domain) || '1a2b' });
		var confirmLocal = E('input', { 'type': 'checkbox' });

		var radios = {};
		[ 'controller_self', 'gateway', 'custom' ].forEach(function(kind) {
			var info = availability[kind] || { enabled: kind === 'custom', reason: null };
			radios[kind] = E('input', {
				'type': 'radio', 'name': 'wifi-source-kind',
				'checked': kind === currentKind ? '' : null,
				'disabled': info.enabled ? null : ''
			});
			if (kind === currentKind && !info.enabled) radios[kind].checked = false;
		});

		var sourceLabels = {
			controller_self: '控制器自身的 Wi-Fi 信息',
			gateway: '网关的 Wi-Fi 信息（只读拉取）',
			custom: '自定义信息'
		};

		var sourceRows = Object.keys(sourceLabels).map(function(kind) {
			var info = availability[kind] || {};
			return E('div', { 'class': 'cbi-value' }, [
				E('label', { 'class': 'cbi-value-title' }, E('span', {}, [ radios[kind], ' ', sourceLabels[kind] ])),
				E('div', { 'class': 'cbi-value-field' }, info.enabled
					? E('span', {}, kind === 'custom'
						? '始终可用（若本设备同时承担 AP，会修改本机无线配置，需勾选下方确认）'
						: '可用')
					: E('span', { 'class': 'label warning' }, info.reason || '不可用'))
			]);
		});

		function saveSource() {
			var kind = Object.keys(radios).filter(function(key) {
				return radios[key].checked;
			})[0];
			if (!kind) {
				message.innerHTML = '';
				message.appendChild(ws.notice('请选择一个可用的 Wi-Fi 信息源', 'error'));
				return;
			}
			return ws.callWifiSourceSet(
				kind,
				endpoint.value.trim(),
				confirmLocal.checked,
				{
					ssid: ssid.value.trim(),
					auth: auth.value.trim(),
					psk_ref: pskRef.value.trim(),
					band: band.value.trim(),
					mobility_domain: domain.value.trim(),
					ieee80211k: true, ieee80211v: true, ieee80211r: true, ft_over_ds: true
				}
			).then(function(result) {
				message.innerHTML = '';
				message.appendChild(ws.notice('Wi-Fi 信息源已保存' +
					(result.requires_confirmation ? '（仍需确认后才能应用到本机）' : ''), 'success'));
			}).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(String(err.message || err), 'error'));
			});
		}

		return E('div', {}, [
			!isController ? ws.notice('本设备未承担 Controller 角色：准入与下发不可用。', 'warning') : null,

			ws.card('新 AP 准入', [
				E('table', { 'class': 'table' }, [
					E('thead', {}, E('tr', {}, [
						E('th', {}, '设备 ID'), E('th', {}, 'MAC'), E('th', {}, '主机名'),
						E('th', {}, '来源'), E('th', {}, '状态'), E('th', {}, '操作')
					])),
					tableBody
				]),
				E('div', { 'class': 'cbi-section-descr' },
					'未通过准入的 AP 不会被下发任何信息，也不会写入本机配置（服务端双重校验）。')
			].filter(Boolean)),

			ws.card('Wi-Fi 信息源', sourceRows.concat([
				ws.kv('网关地址', endpoint),
				ws.notice('若网关上报"没有 Wi-Fi"，该来源会被禁用；控制器自身没有 Wi-Fi 时同理。', 'info'),
				E('h4', {}, '自定义信息（仅 kind = custom 时生效）'),
				ws.kv('SSID', ssid),
				ws.kv('加密方式', auth),
				ws.kv('密钥引用', pskRef),
				ws.kv('频段', band),
				ws.kv('mobility_domain', domain),
				E('div', { 'class': 'cbi-value' }, [
					E('label', { 'class': 'cbi-value-title' },
						E('span', {}, [ confirmLocal, ' 确认可以修改本机无线配置' ])),
					E('div', { 'class': 'cbi-value-field' },
						'当「AP 与 Controller 是同一台设备」且选择自定义信息时，本机 Wi-Fi 也会被修改，必须显式确认。')
				]),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [ ws.submit('保存 Wi-Fi 信息源', saveSource) ])
				]),
				message
			]), 'Controller 只把这些信息下发给已准入的 AP；选择「控制器自身」时不会改写控制器本机配置。')
		].filter(Boolean));
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
