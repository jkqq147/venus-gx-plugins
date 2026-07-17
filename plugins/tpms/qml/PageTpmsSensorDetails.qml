import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	title: root.isChinese ? "传感器详情" : qsTr("Sensor details")

	property string slotPrefix: ""

	model: VisibleItemModel {
		MbItemValue {
			description: root.isChinese ? "传感器 ID" : qsTr("Sensor ID")
			item.bind: root.slotPrefix + "/SensorId"
		}

		MbItemValue {
			description: root.isChinese ? "电量" : qsTr("Battery")
			item {
				bind: root.slotPrefix + "/Battery"
				unit: "%"
			}
		}

		MbItemValue {
			description: qsTr("RSSI")
			item {
				bind: root.slotPrefix + "/Rssi"
				unit: "dBm"
			}
		}

		MbItemValue {
			description: root.isChinese ? "最近接收" : qsTr("Last seen")
			item.bind: root.slotPrefix + "/LastSeen"
		}
	}
}
