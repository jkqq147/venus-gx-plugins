import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	property int serviceIndex: 0
	property string serviceRoot: "com.victronenergy.rathole"
	property string servicePath: root.serviceRoot + "/Services/" + root.serviceIndex
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	property VBusItem nameItem: VBusItem { bind: root.servicePath + "/Name" }
	property VBusItem deleteCommand: VBusItem { bind: root.servicePath + "/Delete" }
	title: nameItem.value === "" ? qsTr("Rathole") : nameItem.value

	model: VisibleItemModel {
		MbItemValue {
			description: root.isChinese ? "服务名称" : qsTr("Server service name")
			item.bind: root.servicePath + "/Name"
		}

		MbEditBox {
			description: root.isChinese ? "名称前缀" : qsTr("Name prefix")
			maximumLength: 32
			item.bind: root.servicePath + "/Slug"
		}

		MbEditBox {
			description: root.isChinese ? "内网地址" : qsTr("Local address")
			maximumLength: 253
			item.bind: root.servicePath + "/Host"
		}

		MbEditBox {
			description: root.isChinese ? "内网端口" : qsTr("Local port")
			maximumLength: 5
			item.bind: root.servicePath + "/Port"
		}

		MbOK {
			description: root.isChinese ? "删除服务" : qsTr("Delete service")
			value: root.isChinese ? "按下选择" : qsTr("Press to choose")
			onClicked: deleteConfirmation.edit()
		}

		MbItemOptions {
			id: deleteConfirmation
			description: root.isChinese ? "确认删除" : qsTr("Confirm deletion")
			message: root.isChinese ? "服务将在保存后删除。" : qsTr("The service will be removed after the configuration is saved.")
			show: false
			possibleValues: [
				MbOption { description: root.isChinese ? "取消" : qsTr("Cancel"); value: 0 },
				MbOption { description: root.isChinese ? "删除" : qsTr("Delete"); value: 1 }
			]
			onOptionSelected: {
				if (newValue === 1) {
					root.deleteCommand.setValue(1)
					pageStack.pop()
				}
			}
		}
	}
}
