'use strict';
'require rpc';

// WifiSync ubus method declarations (mirrors the method table of
// /usr/libexec/rpcd/wifisync). All calls go through ubus -> rpcd -> wifisync,
// no custom protocol is introduced.

var callStatus = rpc.declare({ object: 'wifisync', method: 'status', expect: {} });
var callCapabilities = rpc.declare({ object: 'wifisync', method: 'capabilities', expect: {} });
var callRolesGet = rpc.declare({ object: 'wifisync', method: 'roles_get', expect: {} });
var callRolesSet = rpc.declare({
	object: 'wifisync', method: 'roles_set', params: [ 'roles' ], expect: {}
});
var callBridgePreview = rpc.declare({ object: 'wifisync', method: 'bridge_preview', expect: {} });
var callPlan = rpc.declare({ object: 'wifisync', method: 'plan_dry_run', expect: {} });
var callApply = rpc.declare({ object: 'wifisync', method: 'apply', expect: {} });
var callConfirm = rpc.declare({ object: 'wifisync', method: 'confirm', expect: {} });
var callRevert = rpc.declare({ object: 'wifisync', method: 'revert_last_change', expect: {} });
var callWifiSourceGet = rpc.declare({ object: 'wifisync', method: 'wifi_source_get', expect: {} });
var callWifiSourceSet = rpc.declare({
	object: 'wifisync', method: 'wifi_source_set',
	params: [ 'kind', 'gateway_endpoint', 'local_wifi_change_confirmed', 'custom' ], expect: {}
});
var callGatewaySet = rpc.declare({
	object: 'wifisync', method: 'gateway_set', params: [ 'lan_ifaces' ], expect: {}
});
var callProbe = rpc.declare({
	object: 'wifisync', method: 'probe_connectivity', params: [ 'target' ], expect: {}
});
var callAdmissionList = rpc.declare({ object: 'wifisync', method: 'admission_list', expect: {} });
var callAdmissionApprove = rpc.declare({
	object: 'wifisync', method: 'admission_approve', params: [ 'device_id' ], expect: {}
});
var callAdmissionReject = rpc.declare({
	object: 'wifisync', method: 'admission_reject', params: [ 'device_id' ], expect: {}
});
var callAdmissionRevoke = rpc.declare({
	object: 'wifisync', method: 'admission_revoke', params: [ 'device_id' ], expect: {}
});
var callAdmissionRegister = rpc.declare({
	object: 'wifisync', method: 'admission_register',
	params: [ 'device_id', 'mac', 'hostname' ], expect: {}
});
var callBackupList = rpc.declare({ object: 'wifisync', method: 'backup_list', expect: {} });
var callBackupVerify = rpc.declare({ object: 'wifisync', method: 'backup_verify', params: [ 'path' ], expect: {} });
var callBackupCreate = rpc.declare({ object: 'wifisync', method: 'backup_create', params: [ 'kind' ], expect: {} });
var callBackupPrune = rpc.declare({ object: 'wifisync', method: 'backup_prune', expect: {} });
var callRestore = rpc.declare({ object: 'wifisync', method: 'restore', params: [ 'mode', 'snapshot' ], expect: {} });
var callFailsafeGet = rpc.declare({ object: 'wifisync', method: 'failsafe_get', expect: {} });
var callFailsafeSet = rpc.declare({
	object: 'wifisync', method: 'failsafe_set',
	params: [ 'enabled', 'apply_confirm_secs', 'link_timeout_secs', 'action', 'keep_ssid', 'heartbeat_endpoint' ],
	expect: {}
});
var callVersion = rpc.declare({ object: 'wifisync', method: 'version', expect: {} });
var callLogs = rpc.declare({ object: 'wifisync', method: 'logs_tail', params: [ 'lines' ], expect: {} });
var callLinkGet = rpc.declare({ object: 'wifisync', method: 'link_get', expect: {} });
var callLinkSet = rpc.declare({
	object: 'wifisync', method: 'link_set',
	params: [ 'controller_endpoint', 'controller_username', 'controller_password', 'clear_password', 'controller_port', 'controller_bind' ],
	expect: {}
});
var callLinkStatus = rpc.declare({ object: 'wifisync', method: 'link_status', expect: {} });
var callAccountList = rpc.declare({ object: 'wifisync', method: 'account_list', expect: {} });
var callAccountAdd = rpc.declare({
	object: 'wifisync', method: 'account_add', params: [ 'username', 'password' ], expect: {}
});
var callAccountPasswd = rpc.declare({
	object: 'wifisync', method: 'account_passwd', params: [ 'username', 'password' ], expect: {}
});
var callAccountRemove = rpc.declare({
	object: 'wifisync', method: 'account_remove', params: [ 'username' ], expect: {}
});
var callLanReport = rpc.declare({ object: 'wifisync', method: 'lan_report', expect: {} });
var callLanList = rpc.declare({ object: 'wifisync', method: 'lan_list', expect: {} });

// ── backend messages ─────────────────────────────────────────────────────
//
// The backend sends structured messages ({ key, params }) instead of finished
// sentences. The keys are defined in crates/wifisync-core/src/message.rs and in
// the daemon; both sides are compared by CI. Parameters are `%{name}`.

var MESSAGES = {
	'bridge.blocked': _('The current role combination creates no bridge and touches no port (%{blockers})'),

	'plan.bridge_planned': _('Bridge plan: %{bridge}'),
	'plan.kvr': _('KVR: k=%{k} v=%{v} r=%{r} mobility_domain=%{mobility_domain} ft_over_ds=%{ft_over_ds}'),
	'plan.wifi_not_applied': _('This device does not take the AP role: the Wi-Fi information is only distributed to APs and does not change the local wireless configuration'),
	'plan.profile_referenced': _('Referenced distributed profile %{profile}'),

	'role.ap_removed_no_wifi': _('This device has no wireless module; the AP role was removed and the option is disabled'),

	'failsafe.state.disabled': _('Disabled'),
	'failsafe.state.not_applicable': _('Not applicable (not an AP device)'),
	'failsafe.state.idle': _('Monitoring'),
	'failsafe.state.pending_confirm': _('Waiting for apply confirmation'),
	'failsafe.state.watchdog': _('Waiting for heartbeat recovery'),
	'failsafe.state.tripped': _('Restore triggered'),

	'wifi_source.reason.controller_no_wifi': _('The Controller itself has no Wi-Fi'),
	'wifi_source.reason.gateway_no_wifi': _('The Gateway reports that it has no Wi-Fi'),
	'wifi_source.reason.gateway_not_selected': _('No Gateway selected or connectivity not confirmed yet'),
	'wifi_source.reason.gateway_fetch_pending': _('Wi-Fi information has not been fetched from the Gateway successfully yet'),
	'wifi_source.reason.gateway_fetch_failed': _('Fetching the Wi-Fi information from the Gateway failed'),
	'wifi_source.reason.unknown': _('Unknown source'),
	'wifi_source.reason.unavailable': _('Unavailable'),
	'wifi_source.note.controller_self': _('Source: the Controller\'s own Wi-Fi information (used as a template only, the local configuration of the Controller is not changed)'),
	'wifi_source.note.gateway': _('Source: the Gateway\'s Wi-Fi information (read-only fetch, never written back to the Gateway)'),
	'wifi_source.note.custom': _('Source: user-defined Wi-Fi information'),
	'wifi_source.note.local_wifi_modified': _('Note: this device is also an AP, applying custom Wi-Fi information will modify its local wireless configuration'),
	'wifi_source.kvr.no_radio': _('This device has no wireless module'),
	'wifi_source.kvr.wpad_basic': _('The full wpad package (with 802.11k/v/r) is required: install `wpad` instead of `wpad-basic`'),

	'backup.verify.ok': _('Backup integrity check passed'),
	'backup.verify.failed': _('Backup check failed: %{missing} missing, %{changed} changed, %{extra} extra'),
	'restore.blocked': _('Restore blocked: %{reason}'),
	'restore.done': _('Restored %{restored} item(s), %{failed} failed'),

	'daemon.apply.nothing_to_do': _('This role combination does not need to modify the network (0 changes)'),
	'daemon.apply.applied': _('Applied %{count} change(s)'),
	'daemon.gateway.recorded': _('The Gateway role does not modify any network configuration; the interfaces are only recorded for identification and probing'),
	'daemon.backup.no_snapshot': _('No snapshot yet: start the service first (the initial baseline is created automatically) or create one with `wifisync backup create`'),

	'probe.reachable': _('%{address} is reachable'),
	'probe.unreachable_tcp': _('%{address} is unreachable (TCP connection failed)'),
	'probe.unresolved': _('Cannot resolve %{address}: %{error}'),
	'probe.icmp_raw': _('ICMP reply: %{output}')
};

// Blocker tokens emitted by the backend inside the `bridge.blocked` message.
var BLOCKER_LABELS = {
	gateway: _('Gateway'),
	controller: _('Controller'),
	no_ap: _('the AP role is not enabled'),
	none: _('none')
};

// Parameter formatters for values needing more than plain interpolation.
var PARAM_FORMATTERS = {
	blockers: function(value) {
		return String(value).split(',').map(function(token) {
			return BLOCKER_LABELS[token] || token;
		}).join(' / ');
	}
};

/**
 * Translates a backend message.
 *
 * Accepts a structured message ({ key, params }), a plain string or null.
 * Plain strings that happen to be a known key are translated as well, so errors
 * carrying a key are localized too; unknown strings pass through unchanged.
 */
function message(msg) {
	if (msg == null)
		return '';

	if (typeof(msg) === 'string')
		return (MESSAGES[msg] != null) ? MESSAGES[msg] : msg;

	var template = MESSAGES[msg.key];

	if (template == null)
		return msg.key;

	var params = msg.params || {};

	return template.replace(/%\{(\w+)\}/g, function(match, name) {
		if (!(name in params))
			return match;

		var formatter = PARAM_FORMATTERS[name];
		var value = String(params[name]);

		return formatter ? formatter(value) : value;
	});
}

/**
 * Renders a dry-run plan: translated notes followed by the language neutral uci
 * commands produced by the backend.
 */
function planText(plan) {
	plan = plan || {};

	var notes = (plan.notes || []).map(message).filter(function(line) {
		return line !== '';
	}).map(function(line) {
		return '# ' + line;
	}).join('\n');

	var commands = plan.text || '';

	if (notes && commands)
		return notes + '\n' + commands;

	return notes || commands;
}

/** Turns an RPC rejection into translatable text. */
function errorText(err) {
	return message((err && err.message) ? err.message : err);
}

// ── shared widgets ───────────────────────────────────────────────────────

function boolLabel(value) {
	return value ? E('span', { 'class': 'label success' }, _('Yes'))
	             : E('span', { 'class': 'label' }, _('No'));
}

function roleChip(label) {
	return E('span', { 'class': 'label notice' }, label);
}

function kv(key, value) {
	return E('div', { 'class': 'cbi-value' }, [
		E('label', { 'class': 'cbi-value-title' }, key),
		E('div', { 'class': 'cbi-value-field' }, value)
	]);
}

function card(title, nodes, hint) {
	return E('div', { 'class': 'cbi-section' }, [
		E('h3', {}, title),
		hint ? E('div', { 'class': 'cbi-section-descr' }, hint) : null,
		E('div', { 'class': 'cbi-section-node' }, nodes)
	].filter(Boolean));
}

function notice(text, type) {
	return E('div', { 'class': 'alert-message ' + (type || 'warning') }, text);
}

function errorBox(text) {
	return text ? notice(text, 'error') : null;
}

// ── Controller link and LAN report ─────────────────────────────────────

// State keys of the AP / Gateway client (see crates/wifisync/src/daemon/link_glue.rs).
var LINK_STATES = {
	idle: _('Not used (neither AP nor Gateway role)'),
	not_configured: _('Controller address not configured'),
	connecting: _('Connecting'),
	connected: _('Connected'),
	error: _('Connection failed')
};

var IPV6_MODES = {
	disabled: _('IPv6 disabled on the LAN'),
	slaac: _('SLAAC only'),
	dhcpv6: _('DHCPv6 only'),
	slaac_dhcpv6: _('SLAAC + DHCPv6'),
	dhcpv6_stateful: _('Stateful DHCPv6'),
	relay: _('Relay')
};

function linkStateLabel(state) {
	return LINK_STATES[state] || state || '-';
}

/** Rows describing one LAN report: bridge, IPv4 network, DHCP range and IPv6 policy. */
function lanRows(report) {
	report = report || {};

	var bridge = report.bridge || {}, ipv4 = report.ipv4 || {}, dhcp = report.dhcp || {}, ipv6 = report.ipv6 || {};
	var none = _('none');

	return [
		kv(_('LAN bridge'), E('span', {}, [
			E('code', {}, bridge.name || '-'),
			bridge.interface ? ' (' + bridge.interface + ')' : '',
			' ',
			bridge.present ? '' : E('span', { 'class': 'label warning' }, _('not present'))
		])),
		kv(_('Bridge ports'), E('span', {}, (bridge.ports || []).join(', ') || none)),
		kv(_('IPv4 network'), E('span', {}, ipv4.address
			? ipv4.address + (ipv4.prefix_len != null ? '/' + ipv4.prefix_len : '') +
			  (ipv4.network ? '  (' + ipv4.network + ')' : '')
			: none)),
		kv(_('DHCP server'), dhcp.enabled
			? E('span', {}, (dhcp.first && dhcp.last ? dhcp.first + ' - ' + dhcp.last : _('range unknown')) +
				(dhcp.leasetime ? '  (' + _('lease time') + ' ' + dhcp.leasetime + ')' : ''))
			: E('span', { 'class': 'label' }, _('Disabled'))),
		kv(_('IPv6 policy'), E('span', {}, IPV6_MODES[ipv6.mode] || ipv6.mode || none)),
		kv(_('IPv6 details'), E('span', {}, [
			'DHCPv6: ' + (ipv6.dhcpv6 || '-'),
			'  RA: ' + (ipv6.ra || '-'),
			'  NDP: ' + (ipv6.ndp || '-'),
			ipv6.ip6assign != null ? '  ip6assign: ' + ipv6.ip6assign : '',
			ipv6.ula_prefix ? '  ULA: ' + ipv6.ula_prefix : '',
			ipv6.upstream_proto ? '  ' + _('upstream') + ': ' + ipv6.upstream_proto : ''
		].join('')))
	];
}

function submit(label, handler) {
	return E('button', {
		'class': 'cbi-button cbi-button-apply',
		'click': function(ev) {
			ev.preventDefault();
			handler();
		}
	}, label);
}

return {
	callStatus: callStatus,
	callCapabilities: callCapabilities,
	callRolesGet: callRolesGet,
	callRolesSet: callRolesSet,
	callBridgePreview: callBridgePreview,
	callPlan: callPlan,
	callApply: callApply,
	callConfirm: callConfirm,
	callRevert: callRevert,
	callWifiSourceGet: callWifiSourceGet,
	callWifiSourceSet: callWifiSourceSet,
	callGatewaySet: callGatewaySet,
	callProbe: callProbe,
	callAdmissionList: callAdmissionList,
	callAdmissionApprove: callAdmissionApprove,
	callAdmissionReject: callAdmissionReject,
	callAdmissionRevoke: callAdmissionRevoke,
	callAdmissionRegister: callAdmissionRegister,
	callBackupList: callBackupList,
	callBackupVerify: callBackupVerify,
	callBackupCreate: callBackupCreate,
	callBackupPrune: callBackupPrune,
	callRestore: callRestore,
	callFailsafeGet: callFailsafeGet,
	callFailsafeSet: callFailsafeSet,
	callVersion: callVersion,
	callLogs: callLogs,
	callLinkGet: callLinkGet,
	callLinkSet: callLinkSet,
	callLinkStatus: callLinkStatus,
	callAccountList: callAccountList,
	callAccountAdd: callAccountAdd,
	callAccountPasswd: callAccountPasswd,
	callAccountRemove: callAccountRemove,
	callLanReport: callLanReport,
	callLanList: callLanList,

	linkStateLabel: linkStateLabel,
	lanRows: lanRows,
	message: message,
	planText: planText,
	errorText: errorText,
	boolLabel: boolLabel,
	roleChip: roleChip,
	kv: kv,
	card: card,
	notice: notice,
	errorBox: errorBox,
	submit: submit
};
