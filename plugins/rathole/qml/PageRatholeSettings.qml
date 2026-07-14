import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	title: qsTr("Rathole")
	property string pluginRoot: "com.victronenergy.pluginmanager/Plugins/rathole"
	property string configPath: "/data/venus-gx-plugins/config/rathole/client.toml"
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property VBusItem serviceState: VBusItem { bind: root.pluginRoot + "/ServiceState" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"

	function serviceText(value) {
		if (!root.isChinese)
			return value === "running" ? qsTr("Running") : value === "stopped" ? qsTr("Stopped") : value === "failed" ? qsTr("Failed") : qsTr("Unknown")
		return value === "running" ? "运行中" : value === "stopped" ? "已关闭" : value === "failed" ? "异常" : "未知"
	}

	model: VisibleItemModel {
		MbItemValue {
			description: root.isChinese ? "状态" : qsTr("Status")
			item: VBusItem { value: root.serviceText(root.serviceState.value) }
		}

		MbItemValue {
			description: root.isChinese ? "配置文件" : qsTr("Configuration")
			item: VBusItem { value: root.configPath }
		}

		MbItemText {
			text: root.isChinese
				? "通过 SSH 使用 nano 编辑 client.toml，保存后关闭并重新开启插件。"
				: qsTr("Edit client.toml through SSH with nano. Disable and enable the plugin after saving changes.")
			wrapMode: Text.WordWrap
		}
	}
}
