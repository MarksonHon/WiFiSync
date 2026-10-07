'use strict';
'require view';
'require ui';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return Promise.all([
			ws.callAdmissionList(),
			ws.callWifiSourceGet(),
			ws.callStatus(),
			ws.callLinkGet(),
			ws.callLinkStatus(),
			ws.callAccountList(),
			ws.callLanList()
		]);
	},

	render: function(data) {
		var admission = data[0] || {}, source = data[1] || {}, status = data[2] || {};
		var link = data[3] || {}, linkStatus = data[4] || {}, accounts = (data[5] || {}).accounts || [];
		var lans = (data[6] || {}).entries || [];
		var listener = linkStatus.controller || {};
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
			if (!confirm(label + ' ' + deviceId + ' ?')) return;
			method(deviceId).then(refresh).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(ws.errorText(err), 'error'));
			});
		}

		var tableBody = E('tbody', {});
		function renderTable() {
			tableBody.innerHTML = '';
			var entries = admission.entries || [];
			if (!entries.length) {
				tableBody.appendChild(E('tr', {}, [
					E('td', { 'colspan': 6, 'style': 'text-align:center' }, _('No device has reported yet'))
				]));
				return;
			}
			entries.forEach(function(entry) {
				var stateLabel = {
					pending: E('span', { 'class': 'label warning' }, _('Pending')),
					approved: E('span', { 'class': 'label success' }, _('Approved')),
					rejected: E('span', { 'class': 'label' }, _('Rejected'))
				}[entry.state] || entry.state;

				tableBody.appendChild(E('tr', {}, [
					E('td', {}, entry.device_id.slice(0, 8)),
					E('td', {}, entry.mac || '-'),
					E('td', {}, entry.hostname || '-'),
					E('td', {}, entry.source_addr || '-'),
					E('td', {}, stateLabel),
					E('td', {}, [
						ws.submit(_('Approve'), function() { setState(ws.callAdmissionApprove, entry.device_id, _('Approve device')); }),
						' ',
						ws.submit(_('Reject'), function() { setState(ws.callAdmissionReject, entry.device_id, _('Reject device')); }),
						' ',
						ws.submit(_('Revoke'), function() { setState(ws.callAdmissionRevoke, entry.device_id, _('Revoke approval of device')); })
					])
				]));
			});
		}
		renderTable();

		// ── Controller link: listen address / port ──────────────────────────
		var bindInput = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': link.controller_bind || '0.0.0.0' });
		var portInput = E('input', { 'type': 'number', 'class': 'cbi-input-text', 'min': 1, 'max': 65535,
			'style': 'width:7em', 'value': link.controller_port || link.default_port || 6550 });
		var linkMessage = E('div', {});

		function saveLink() {
			return ws.callLinkSet(null, null, null, null, parseInt(portInput.value, 10), bindInput.value.trim())
				.then(function() {
					linkMessage.innerHTML = '';
					linkMessage.appendChild(ws.notice(_('Controller listener saved; it is restarted within a second'), 'success'));
				}).catch(function(err) {
					linkMessage.innerHTML = '';
					linkMessage.appendChild(ws.notice(ws.errorText(err), 'error'));
				});
		}

		// ── Accounts ────────────────────────────────────────────────────────
		var accountBody = E('tbody', {});
		var accountMessage = E('div', {});
		var accountName = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'autocomplete': 'off' });
		var accountPassword = E('input', { 'type': 'password', 'class': 'cbi-input-text', 'autocomplete': 'new-password' });

		function accountDone(text) {
			accountPassword.value = '';
			return ws.callAccountList().then(function(next) {
				accounts = (next || {}).accounts || [];
				renderAccounts();
				accountMessage.innerHTML = '';
				accountMessage.appendChild(ws.notice(text, 'success'));
			});
		}

		function accountFailed(err) {
			accountMessage.innerHTML = '';
			accountMessage.appendChild(ws.notice(ws.errorText(err), 'error'));
		}

		function renderAccounts() {
			accountBody.innerHTML = '';
			if (!accounts.length) {
				accountBody.appendChild(E('tr', {}, E('td', { 'colspan': 3, 'style': 'text-align:center' },
					_('No account yet: APs and Gateways on other devices cannot connect'))));
				return;
			}
			accounts.forEach(function(account) {
				accountBody.appendChild(E('tr', {}, [
					E('td', {}, E('a', { 'href': '#', 'click': function(ev) {
						ev.preventDefault();
						accountName.value = account.username;
					} }, account.username)),
					E('td', {}, new Date(account.created_at * 1000).toLocaleString()),
					E('td', {}, ws.submit(_('Delete'), function() {
						if (!confirm(_('Delete account') + ' ' + account.username + ' ?')) return;
						ws.callAccountRemove(account.username).then(function() {
							return accountDone(_('Account deleted'));
						}).catch(accountFailed);
					}))
				]));
			});
		}
		renderAccounts();

		// ── Gateway LAN reports ─────────────────────────────────────────────
		var lanCards = lans.map(function(entry) {
			return ws.card((entry.hostname || entry.device_id.slice(0, 8)) +
				(entry.source_addr ? '  [' + entry.source_addr + ']' : ''),
				ws.lanRows(entry.report).concat([
					ws.kv(_('Reported at'), E('span', {}, new Date(entry.reported_at * 1000).toLocaleString()))
				]));
		});

		// ── Wi-Fi information sources ───────────────────────────────────────
		var availability = {};
		(source.availability || []).forEach(function(item) { availability[item.kind] = item; });
		var currentKind = (source.config && source.config.kind) || 'controller_self';
		var endpoint = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'style': 'width:20em',
			'value': (source.config && source.config.gateway_endpoint) || '' });

		var custom = (source.config && source.config.custom) || {};
		var ssid = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': custom.ssid || '' });
		var auth = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': custom.auth || 'sae-mixed' });
		var pskRef = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': custom.psk_ref || 'custom' });
		var band = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': custom.band || '5g' });
		var domain = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'value': (custom.kvr && custom.kvr.mobility_domain) || '' });
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
			controller_self: _('Wi-Fi information of the Controller itself'),
			gateway: _('Wi-Fi information of the Gateway (read-only fetch)'),
			custom: _('Custom information')
		};

		var sourceRows = Object.keys(sourceLabels).map(function(kind) {
			var info = availability[kind] || {};
			return E('div', { 'class': 'cbi-value' }, [
				E('label', { 'class': 'cbi-value-title' }, E('span', {}, [ radios[kind], ' ', sourceLabels[kind] ])),
				E('div', { 'class': 'cbi-value-field' }, info.enabled
					? E('span', {}, kind === 'custom'
						? _('Always available (if this device is also an AP, its own wireless configuration is modified and the confirmation below is required)')
						: _('Available'))
					: E('span', { 'class': 'label warning' }, ws.message(info.reason) || _('Unavailable')))
			]);
		});

		function saveSource() {
			var kind = Object.keys(radios).filter(function(key) {
				return radios[key].checked;
			})[0];
			if (!kind) {
				message.innerHTML = '';
				message.appendChild(ws.notice(_('Please select an available Wi-Fi information source'), 'error'));
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
				message.appendChild(ws.notice(_('Wi-Fi information source saved') +
					(result.requires_confirmation ? ' ' + _('(it still has to be confirmed before being applied locally)') : ''), 'success'));
			}).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(ws.errorText(err), 'error'));
			});
		}

		return E('div', {}, [
			!isController ? ws.notice(_('This device does not take the Controller role: admission and distribution are unavailable.'), 'warning') : null,

			ws.card(_('New AP admission'), [
				E('table', { 'class': 'table' }, [
					E('thead', {}, E('tr', {}, [
						E('th', {}, _('Device ID')), E('th', {}, _('MAC')), E('th', {}, _('Hostname')),
						E('th', {}, _('Source')), E('th', {}, _('State')), E('th', {}, _('Actions'))
					])),
					tableBody
				]),
				E('div', { 'class': 'cbi-section-descr' },
					_('An AP that has not passed admission receives no information and writes no local configuration (validated twice on the service side).'))
			].filter(Boolean)),

			ws.card(_('Controller link'), [
				ws.kv(_('Listening on'), listener.listening && listener.listening.length
					? E('span', {}, listener.listening.join(', '))
					: E('span', { 'class': 'label warning' }, listener.error || _('not listening'))),
				ws.kv(_('Listen address'), bindInput),
				ws.kv(_('Port'), portInput),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [ ws.submit(_('Save listener'), saveLink) ])
				]),
				linkMessage
			], _('AP and Gateway devices connect here (TCP, port 6550 by default) and log in with an account below. Devices on this same device connect over loopback and need no account. Remember to allow the port in the firewall if the devices are in another zone.')),

			ws.card(_('Accounts'), [
				E('table', { 'class': 'table' }, [
					E('thead', {}, E('tr', {}, [ E('th', {}, _('Account')), E('th', {}, _('Created')), E('th', {}, _('Actions')) ])),
					accountBody
				]),
				ws.kv(_('Account'), accountName),
				ws.kv(_('Password'), accountPassword),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit(_('Create account'), function() {
							ws.callAccountAdd(accountName.value.trim(), accountPassword.value)
								.then(function() { return accountDone(_('Account created')); }).catch(accountFailed);
						}),
						' ',
						ws.submit(_('Change password'), function() {
							ws.callAccountPasswd(accountName.value.trim(), accountPassword.value)
								.then(function() { return accountDone(_('Password changed')); }).catch(accountFailed);
						})
					])
				]),
				accountMessage
			], _('The password must have at least 8 characters. It is never sent over the network: devices prove that they know it, and everything after the login is encrypted.')),

			ws.card(_('LAN information from Gateways'), lanCards.length ? lanCards : [
				ws.notice(_('No Gateway has reported yet'), 'info')
			], _('Gateways report the bridge that carries the LAN, its network, the DHCP range and the IPv6 policy. The Controller forwards this information to admitted APs.')),

			ws.card(_('Wi-Fi information source'), sourceRows.concat([
				ws.kv(_('Gateway address'), endpoint),
				ws.notice(_('If the Gateway reports "no Wi-Fi", that source is disabled; the same applies when the Controller itself has no Wi-Fi.'), 'info'),
				E('h4', {}, _('Custom information (only effective when kind = custom)')),
				ws.kv('SSID', ssid),
				E('div', { 'class': 'cbi-section-descr' },
					_('Leave blank to generate a name automatically (Home_Wi-Fi_xxxxxx)')),
				ws.kv(_('Encryption'), auth),
				ws.kv(_('Key reference'), pskRef),
				ws.kv(_('Band'), band),
				ws.kv('mobility_domain', domain),
				E('div', { 'class': 'cbi-section-descr' },
					_('Leave blank to derive it from the SSID (4 hex digits otherwise)')),
				E('div', { 'class': 'cbi-value' }, [
					E('label', { 'class': 'cbi-value-title' },
						E('span', {}, [ confirmLocal, ' ', _('Confirm that the local wireless configuration may be modified') ])),
					E('div', { 'class': 'cbi-value-field' },
						_('When the AP and the Controller are the same device and custom information is selected, the local Wi-Fi is modified as well, which must be confirmed explicitly.'))
				]),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [ ws.submit(_('Save Wi-Fi information source'), saveSource) ])
				]),
				message
			]), _('The Controller only distributes this information to admitted APs; choosing "the Controller itself" never rewrites the local configuration of the Controller.'))
		].filter(Boolean));
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
