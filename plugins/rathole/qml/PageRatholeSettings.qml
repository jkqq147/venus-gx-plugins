import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	title: qsTr("Rathole")
	property string serviceRoot: "com.victronenergy.rathole"
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	property VBusItem statusState: VBusItem { bind: root.serviceRoot + "/Status/State" }
	property VBusItem statusTextItem: VBusItem { bind: root.serviceRoot + "/Status/Text" }
	property VBusItem configMode: VBusItem { bind: root.serviceRoot + "/Config/Mode" }
	property VBusItem feedback: VBusItem { bind: root.serviceRoot + "/Config/Feedback" }
	property VBusItem feedbackTextItem: VBusItem { bind: root.serviceRoot + "/Config/FeedbackText" }
	property VBusItem dirty: VBusItem { bind: root.serviceRoot + "/Config/Dirty" }
	property VBusItem deviceName: VBusItem { bind: root.serviceRoot + "/Config/DeviceName" }
	property VBusItem originalDeviceName: VBusItem { bind: root.serviceRoot + "/Config/OriginalDeviceName" }
	property VBusItem serviceCount: VBusItem { bind: root.serviceRoot + "/Services/Count" }
	property VBusItem saveCommand: VBusItem { bind: root.serviceRoot + "/Config/Save" }
	property VBusItem confirmRenameCommand: VBusItem { bind: root.serviceRoot + "/Config/ConfirmRename" }
	property VBusItem tokenCommand: VBusItem { bind: root.serviceRoot + "/Config/GenerateToken" }
	property bool isEditable: configMode.value === "managed" || configMode.value === "missing"

	function text(zh, en) {
		return root.isChinese ? zh : qsTr(en)
	}

	function statusText(state) {
		if (!root.isChinese)
			return root.statusTextItem.value
		if (state === "running") return "运行中"
		if (state === "starting") return "正在启动"
		if (state === "restarting") return "正在重启"
		if (state === "not-configured") return "尚未配置"
		if (state === "stopped") return "已停止"
		if (state === "failed") return "运行异常"
		if (state === "invalid-config") return "配置无效"
		return root.statusTextItem.value
	}

	function feedbackText(state) {
		if (!root.isChinese)
			return state === "invalid-input" || state === "rename-confirmation" || state === "read-only"
				? root.feedbackTextItem.value : ""
		if (state === "invalid-input") return "配置内容无效，请检查输入。"
		if (state === "rename-confirmation") return "更改设备名称后，服务端也必须同步更新。"
		if (state === "read-only") return "当前配置只能通过 SSH 管理。"
		return ""
	}

	function requestSave() {
		if (root.originalDeviceName.value !== ""
				&& root.originalDeviceName.value !== root.deviceName.value
				&& root.serviceCount.value > 0)
			renameConfirmation.edit()
		else
			root.saveCommand.setValue(1)
	}

	model: VisibleItemModel {
		MbItemValue {
			description: root.text("状态", "Status")
			item: VBusItem { value: root.statusText(root.statusState.value) }
		}

		MbItemText {
			text: root.feedbackText(root.feedback.value)
			wrapMode: Text.WordWrap
			show: text !== ""
		}

		MbItemText {
			text: root.text(
				"当前 client.toml 包含 GUI 不支持的高级选项。插件会继续按原配置运行，请通过 SSH 管理。",
				"The current client.toml contains advanced options that the GUI cannot preserve. It remains active and must be managed through SSH."
			)
			wrapMode: Text.WordWrap
			show: root.configMode.value === "advanced"
		}

		MbItemText {
			text: root.text(
				"client.toml 无效。请通过 SSH 修复配置后重新启用插件。",
				"client.toml is invalid. Repair it through SSH, then re-enable the plugin."
			)
			wrapMode: Text.WordWrap
			show: root.configMode.value === "invalid"
		}

		MbEditBox {
			description: root.text("服务端地址", "Server address")
			maximumLength: 253
			item.bind: root.serviceRoot + "/Config/Host"
			show: root.isEditable
		}

		MbEditBox {
			description: root.text("控制端口", "Control port")
			maximumLength: 5
			item.bind: root.serviceRoot + "/Config/Port"
			show: root.isEditable
		}

		MbEditBox {
			description: root.text("设备名称", "Device name")
			maximumLength: 24
			item.bind: root.serviceRoot + "/Config/DeviceName"
			show: root.isEditable
		}

		MbItemValue {
			description: root.text("设备 Token", "Device token")
			item.bind: root.serviceRoot + "/Config/Token"
			show: root.isEditable
		}

		MbOK {
			description: root.text("重新生成 Token", "Regenerate token")
			value: root.text("按下选择", "Press to choose")
			show: root.isEditable
			onClicked: tokenConfirmation.edit()
		}

		MbSubMenu {
			description: root.text("转发服务", "Forwarded services")
			show: root.isEditable
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: root.serviceRoot + "/Services/Count"; width: 72; height: 25 }
			subpage: Component { PageRatholeServices {} }
		}

		MbOK {
			description: root.text("保存并应用", "Save and apply")
			value: root.text("按下确认", "Press to confirm")
			show: root.isEditable && root.dirty.value === 1
			onClicked: root.requestSave()
		}

		MbItemOptions {
			id: tokenConfirmation
			description: root.text("确认更换 Token", "Confirm token change")
			message: root.text("更换后，服务端也必须使用新的 Token。", "The server must also be updated to use the new token.")
			show: false
			possibleValues: [
				MbOption { description: root.text("取消", "Cancel"); value: 0 },
				MbOption { description: root.text("生成新 Token", "Generate new token"); value: 1 }
			]
			onOptionSelected: if (newValue === 1) root.tokenCommand.setValue(1)
		}

		MbItemOptions {
			id: renameConfirmation
			description: root.text("确认更改设备名称", "Confirm device rename")
			message: root.text("所有服务名称都会改变，服务端必须同步更新。", "All service names will change and the server must be updated to match.")
			show: false
			possibleValues: [
				MbOption { description: root.text("取消", "Cancel"); value: 0 },
				MbOption { description: root.text("保存更改", "Save changes"); value: 1 }
			]
			onOptionSelected: if (newValue === 1) root.confirmRenameCommand.setValue(1)
		}
	}
}
