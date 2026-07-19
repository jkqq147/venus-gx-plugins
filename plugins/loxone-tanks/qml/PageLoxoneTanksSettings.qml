import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	property string serviceRoot: "com.victronenergy.loxonetanks"
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	property VBusItem connectionState: VBusItem { bind: root.serviceRoot + "/Connection/State" }
	property VBusItem connectionStatus: VBusItem { bind: root.serviceRoot + "/Connection/StatusText" }
	property VBusItem scanState: VBusItem { bind: root.serviceRoot + "/Discovery/State" }
	property VBusItem discoveredCount: VBusItem { bind: root.serviceRoot + "/Discovery/Count" }
	property VBusItem scanCommand: VBusItem { bind: root.serviceRoot + "/Discovery/Scan" }
	property VBusItem passwordCommand: VBusItem { bind: root.serviceRoot + "/Config/Password" }
	property VBusItem saveCommand: VBusItem { bind: root.serviceRoot + "/Config/SaveServer" }
	property VBusItem retryCommand: VBusItem { bind: root.serviceRoot + "/Connection/Retry" }

	title: qsTr("Loxone Tanks")

	function text(zh, en) {
		return root.isChinese ? zh : qsTr(en)
	}

	function connectionText(state) {
		if (!root.isChinese)
			return connectionStatus.value
		if (state === "not-configured") return "尚未配置"
		if (state === "credentials-required") return "请输入账户和密码"
		if (state === "selected") return "已选择 Miniserver"
		if (state === "invalid-address") return "地址无效"
		if (state === "invalid-username") return "用户名无效"
		if (state === "connecting") return "正在连接"
		if (state === "reconnecting") return "连接中断，正在自动重连"
		if (state === "authenticating") return "正在验证账户"
		if (state === "connected") return "已连接"
		if (state === "connection-failed") return "无法连接 Miniserver"
		if (state === "authentication-failed") return "账户或密码错误"
		if (state === "sensor-error") return "未找到完整的水箱 Sensor"
		if (state === "disconnected") return "连接已断开，请手动重连"
		return connectionStatus.value
	}

	function scanText(state) {
		if (state === "scanning") return root.text("扫描中", "Scanning")
		if (state === "complete") return root.text("扫描完成", "Scan complete")
		if (state === "not-found") return root.text("未发现", "Not found")
		return root.text("按下开始扫描", "Press to scan")
	}

	function canReconnect(state) {
		return state === "disconnected" || state === "connection-failed"
	}

	model: VisibleItemModel {
		MbItemText {
			text: root.text(
				"读取 Loxone 中的清水、灰水和黑水液位，并显示在 GX 水箱页面。",
				"Reads fresh, gray and black water levels from Loxone and shows them in the GX tank view."
			)
			wrapMode: Text.WordWrap
		}

		MbItemValue {
			description: root.text("连接状态", "Connection")
			item: VBusItem { value: root.connectionText(root.connectionState.value) }
		}

		MbEditBox {
			description: root.text("Miniserver 地址", "Miniserver address")
			maximumLength: 253
			item.bind: root.serviceRoot + "/Config/Host"
		}

		MbOK {
			description: root.text("查找 Miniserver", "Find Miniserver")
			value: root.scanText(root.scanState.value)
			onClicked: if (root.scanState.value !== "scanning") root.scanCommand.setValue(1)
		}

		MbSubMenu {
			description: root.text("选择扫描结果", "Choose scan result")
			show: root.discoveredCount.valid && root.discoveredCount.value > 0
			subpage: Component { PageLoxoneDiscovery {} }
		}

		MbEditBox {
			description: root.text("只读用户名", "Read-only username")
			maximumLength: 64
			item.bind: root.serviceRoot + "/Config/Username"
		}

			MbEditBox {
				id: passwordInput
				description: root.text("密码", "Password")
				maximumLength: 128
				textInput.text: passwordInput.editMode
					? Array(passwordInput._editText.length + 1).join("*")
					: ""
				onEditDone: {
				root.passwordCommand.setValue(newValue)
				item.value = ""
			}
		}

		MbOK {
			description: root.text("保存并连接", "Save and connect")
			value: root.text("按下确认", "Press to confirm")
			onClicked: root.saveCommand.setValue(1)
		}

		MbOK {
			description: root.text("重新连接", "Reconnect")
			value: root.text("按下重连", "Press to reconnect")
			show: root.canReconnect(root.connectionState.value)
			onClicked: root.retryCommand.setValue(1)
		}

	}
}
