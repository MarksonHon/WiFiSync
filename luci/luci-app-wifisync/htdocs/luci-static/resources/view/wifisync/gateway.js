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
					'出厂角色：' + port.kind + '　DSA：' + (port.dsa ? '是' : '否') +
					'　载波：' + (port.carrier ? '有' : '无'))
			]);
		});

		var message = E('div', {});
		var endpoint = E('input', { 'type': 'text', 'class': 'cbi-input-text', 'style': 'width:20em',
			'placeholder': '如 192.168.1.1 或 192.168.1.1:443' });
		var probeResult = E('div', {});

		function save() {
			var interfaces = Object.keys(selected).filter(function(key) { return selected[key]; });
			return ws.callGatewaySet(interfaces).then(function(result) {
				message.innerHTML = '';
				message.appendChild(ws.notice(result.message || '已保存', 'success'));
				if (result.network_modified)
					message.appendChild(ws.notice('注意：检测到本机写入计划非空，请到「状态总览」确认！', 'error'));
			}).catch(function(err) {
				message.innerHTML = '';
				message.appendChild(ws.notice(String(err.message || err), 'error'));
			});
		}

		function probe() {
			var target = endpoint.value.trim();
			if (!target) return;
			ws.callProbe(target).then(function(result) {
				probeResult.innerHTML = '';
				probeResult.appendChild(ws.notice(
					target + '：' + (result.reachable ? '可达' : '不可达') +
					'（方法 ' + result.method + '，耗时 ' + (result.latency_ms !== null ? result.latency_ms + 'ms' : '-') + '）' +
					'  ' + (result.detail || ''),
					result.reachable ? 'success' : 'warning'));
			});
		}

		return E('div', {}, [
			ws.notice('Gateway 角色不会代替你修改任何网络：这里只选择「对应的 LAN 接口」，' +
				'用于拓扑识别与连通性探测。程序不会改动路由、NAT、防火墙或任何 /etc/config/network 内容。', 'info'),

			ws.card('对应 LAN 接口', rows.length ? rows : [
				ws.notice('没有探测到物理网口（可能是宿主机/虚拟环境）。', 'warning')
			], '选定后请到「状态总览」点一次 dry-run，确认改动数为 0。'),

			ws.card('连通性探测（只读）', [
				E('div', { 'class': 'cbi-value' }, [
					E('label', { 'class': 'cbi-value-title' }, '探测目标'),
					E('div', { 'class': 'cbi-value-field' }, [ endpoint, ' ', ws.submit('探测', probe) ])
				]),
				probeResult,
				E('div', { 'class': 'cbi-section-descr' },
					'Controller 与 Gateway 是否连通由你单独确认：程序只做 ICMP/TCP 探测，' +
					'不会写路由表、不改防火墙、不动 NAT。')
			]),

			ws.card('保存', [
				E('div', { 'class': 'cbi-value' }, [
					E('div', { 'class': 'cbi-value-field' }, [ ws.submit('保存 LAN 接口选择', save) ])
				]),
				message
			])
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
