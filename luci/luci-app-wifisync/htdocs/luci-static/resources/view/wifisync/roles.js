'use strict';
'require view';
'require ui';
'require view.wifisync.common as ws';

return view.extend({
	load: function() {
		return ws.callRolesGet();
	},

	render: function(roles) {
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
				preview.textContent = plan.text;
			}).catch(function(err) {
				preview.textContent = String(err.message || err);
			});
		}

		function save() {
			var value = Object.keys(selected)
				.filter(function(key) { return selected[key]; })
				.join(' ');
			return ws.callRolesSet(value).then(function(result) {
				message.innerHTML = '';
				message.appendChild(ws.notice('角色已保存：' + (result.value || '空') +
					(result.bridge_enabled ? '（将自动建桥）' : '（不会建桥）'), 'success'));
				(result.adjustments || []).forEach(function(text) {
					message.appendChild(ws.notice(text, 'warning'));
				});
				return updatePreview();
			}).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(String(err.message || err), 'error'));
			});
		}

		updatePreview();

		return E('div', {}, [
			ws.card('角色设置', [
				!apAllowed ? ws.notice('本设备没有无线模块：AP 角色已被禁用，默认角色中也不包含 AP。', 'warning') : null,
				checkbox('controller', 'Controller（控制器）',
					'只做两件事：① 新 AP 准入验证；② 网络信息下发。不改动本机任何网络配置。'),
				checkbox('ap', 'AP（接入点）', apAllowed
					? '同步 Controller 下发的网络信息（含 Wi-Fi）。仅"纯 AP"（无 Controller、无 Gateway）才会把所有网口并入 br-lan。'
					: '本设备无无线模块，不可承担 AP 角色。', !apAllowed),
				checkbox('gateway', 'Gateway（网关）',
					'只要求选择对应的 LAN 接口用于识别与探测，绝不修改路由、NAT、防火墙等任何网络配置。'),
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [
						ws.submit('保存并查看影响', save)
					])
				]),
				message
			].filter(Boolean), '建桥规则：enable_bridge = ap && !gateway && !controller'),

			ws.card('改动预览', [ preview ], '保存角色后，这里显示相应角色组合会做哪些改动。')
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
