import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	property string serviceRoot: "com.victronenergy.rathole"
	property string addRoot: root.serviceRoot + "/Services/Add"
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	property VBusItem slugItem: VBusItem { bind: root.addRoot + "/Slug" }
	property VBusItem hostItem: VBusItem { bind: root.addRoot + "/Host" }
	property VBusItem portItem: VBusItem { bind: root.addRoot + "/Port" }
	property VBusItem addCommand: VBusItem { bind: root.addRoot + "/Commit" }
	title: root.isChinese ? "添加服务" : qsTr("Add service")

	model: VisibleItemModel {
		MbItemOptions {
			description: root.isChinese ? "服务类型" : qsTr("Service type")
			bind: root.addRoot + "/Preset"
			possibleValues: [
				MbOption { description: "Home Assistant"; value: "homeassistant" },
				MbOption { description: "Loxone"; value: "loxone" },
				MbOption { description: "Hikvision"; value: "hikvision" },
				MbOption { description: "Frigate"; value: "frigate" },
				MbOption { description: "SSH"; value: "ssh" },
				MbOption { description: root.isChinese ? "自定义" : qsTr("Custom"); value: "custom" }
			]
		}

		MbEditBox {
			description: root.isChinese ? "名称前缀" : qsTr("Name prefix")
			maximumLength: 32
			item.bind: root.addRoot + "/Slug"
		}

		MbEditBox {
			description: root.isChinese ? "内网地址" : qsTr("Local address")
			maximumLength: 253
			item.bind: root.addRoot + "/Host"
		}

		MbEditBox {
			description: root.isChinese ? "内网端口" : qsTr("Local port")
			maximumLength: 5
			item.bind: root.addRoot + "/Port"
		}

		MbItemText {
			text: root.isChinese ? "公网端口由管理员在 Rathole 服务端配置。" : qsTr("The public port is assigned by the Rathole server administrator.")
			wrapMode: Text.WordWrap
		}

		MbOK {
			description: root.isChinese ? "添加" : qsTr("Add")
			value: root.isChinese ? "按下确认" : qsTr("Press to confirm")
			enabled: root.slugItem.value !== "" && root.hostItem.value !== "" && root.portItem.value !== ""
			onClicked: {
				root.addCommand.setValue(1)
				pageStack.pop()
			}
		}
	}
}
