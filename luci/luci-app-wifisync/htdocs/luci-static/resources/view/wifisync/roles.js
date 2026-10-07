'use strict';
'require view';
'require ui';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return Promise.all([ ws.callRolesGet(), ws.callLinkGet(), ws.callLinkStatus() ]);
	},

	render: function(data) {
		var roles = data[0] || {}, link = data[1] || {}, linkStatus = data[2] || {};
		var selected = {
			controller: !!(roles.roles && roles.roles.controller),
			ap: !!(roles.roles && roles.roles.ap),
			gateway: !!(roles.roles && roles.roles.gateway)
		};
		var apAllowed = roles.ap_allowed !== false;

		var checkboxes = {};
		function checkbox(name, title, description, disabled) {
			var input = E('input', {
				'type': 'checkbox',
				'checked': selected[name] ? '' : null,
				'disabled': disabled ? '' : null,
				'change': function(ev) {
					selected[name] = ev.target.checked;
					updatePreview();
				}
			});
			if (disabled) input.checked = false;
			checkboxes[name] = input;
			return E('div', { 'class': 'cbi-value' }, [
				E('label', { 'class': 'cbi-value-title' },
					E('span', {}, [ input, ' ', title ])),
				E('div', { 'class': 'cbi-value-field' }, description)
			]);
		}

		var preview = E('pre', { 'style': 'white-space:pre-wrap' }, '');
		var message = E('div', {});

		function updatePreview() {
			return ws.callPlan().then(function(plan) {
				preview.textContent = ws.planText(plan) || _('(no changes)');
			}).catch(function(err) {
				preview.textContent = ws.errorText(err);
			});
		}

		function save() {
			var value = Object.keys(selected)
				.filter(function(key) { return selected[key]; })
				.join(' ');
			return ws.callRolesSet(value).then(function(result) {
				message.innerHTML = '';
				message.appendChild(ws.notice(
					_('Roles saved') + ': ' + (result.value || _('empty')) + ' ' +
					(result.bridge_enabled ? _('(a bridge will be created)') : _('(no bridge will be created)')),
					'success'));
				(result.adjustments || []).forEach(function(text) {
					message.appendChild(ws.notice(ws.message(text), 'warning'));
				});
				return updatePreview();
			}).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(ws.errorText(err), 'error'));
			});
		}

		updatePreview();

		// ── Link to the Controller (AP / Gateway side) ─────────────────────
		var client = (linkStatus.client && linkStatus.client.status) || {};
		var linkEndpoint = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'style': 'width:20em',
			'value': link.controller_endpoint || '', 'placeholder': _('empty = this device is the Controller') });
		var linkUser = E('input', { 'type': 'text', 'class': 'cbi-input-text',
			'value': link.controller_username || '', 'autocomplete': 'off' });
		var linkPassword = E('input', { 'type': 'password', 'class': 'cbi-input-text', 'autocomplete': 'new-password',
			'placeholder': link.password_set ? _('unchanged') : '' });
		var linkMessage = E('div', {});

		function saveLink(clear) {
			var password = linkPassword.value;
			return ws.callLinkSet(
				linkEndpoint.value.trim(), linkUser.value.trim(), password, clear === true,
				null, null
			).then(function() {
				linkPassword.value = '';
				linkMessage.innerHTML = '';
				linkMessage.appendChild(ws.notice(_('Controller link saved'), 'success'));
			}).catch(function(err) {
				linkMessage.innerHTML = '';
				linkMessage.appendChild(ws.notice(ws.errorText(err), 'error'));
			});
		}

		return E('div', {}, [
			ws.card(_('Role settings'), [
				!apAllowed ? ws.notice(_('This device has no wireless module: the AP role is disabled and is not part of the default roles.'), 'warning') : null,
				checkbox('controller', _('Controller'),
					_('Does only two things: (1) validate new AP admission; (2) push network information. It never modifies any local network configuration.')),
				checkbox('ap', _('AP (access point)'), apAllowed
					? _('Syncs the network information pushed by the Controller (including Wi-Fi). Only a "pure AP" (without Controller and Gateway) merges all ports into br-lan.')
					: _('This device has no wireless module and cannot take the AP role.'), !apAllowed),
				checkbox('gateway', _('Gateway'),
					_('Only asks you to select the corresponding LAN interface for identification and probing; it never modifies routes, NAT, firewall or any other network configuration.')),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit(_('Save and show impact'), save)
					])
				]),
				message
			].filter(Boolean), _('Bridge rule: enable_bridge = ap && !gateway && !controller')),

			ws.card(_('Change preview'), [ preview ], _('After saving the roles, this shows what the selected role combination would change.')),

			ws.card(_('Link to the Controller'), [
				ws.kv(_('State'), E('span', {}, [
					ws.linkStateLabel(client.state),
					client.auth ? '  (' + (client.auth === 'loopback' ? _('same device, no login needed') : _('logged in')) + ')' : ''
				])),
				client.last_error ? ws.kv(_('Last error'), E('span', {}, client.last_error)) : null,
				ws.kv(_('Controller address'), linkEndpoint),
				ws.kv(_('Account'), linkUser),
				ws.kv(_('Password'), linkPassword),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit(_('Save Controller link'), function() { return saveLink(false); }),
						' ',
						ws.submit(_('Forget password'), function() { return saveLink(true); })
					])
				]),
				linkMessage
			].filter(Boolean), _('AP and Gateway devices connect to the Controller on port %d by default. An account created on the Controller is needed, except when this device is the Controller itself (loopback).').format(link.default_port || 6550))
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
