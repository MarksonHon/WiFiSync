'use strict';
'require rpc';

// WifiSync 的 ubus 方法声明（对应 /usr/libexec/rpcd/wifisync 的方法表）。
// 所有调用都走 ubus → rpcd → wifisync，不引入任何自定义协议。

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

// ── 小工具 ────────────────────────────────────────────────────────────────

function boolLabel(value) {
	return value ? E('span', { 'class': 'label success' }, '是')
	             : E('span', { 'class': 'label' }, '否');
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

function errorBox(message) {
	return message ? notice(message, 'error') : null;
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

	boolLabel: boolLabel,
	roleChip: roleChip,
	kv: kv,
	card: card,
	notice: notice,
	errorBox: errorBox,
	submit: submit
};
