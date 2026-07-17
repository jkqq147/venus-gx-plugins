import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	title: root.isChinese ? "绑定 TPMS" : qsTr("Bind TPMS")

	property int sensorIndex: 0
	property string discoveredPrefix: "com.victronenergy.tpms.main/Discovered/" + sensorIndex

	model: VisibleItemModel {
		MbItemValue {
			description: root.isChinese ? "名称" : qsTr("Name")
			item.bind: root.discoveredPrefix + "/Name"
		}

		MbItemValue {
			description: root.isChinese ? "传感器 ID" : qsTr("Sensor ID")
			item.bind: root.discoveredPrefix + "/SensorId"
		}

		MbItemValue {
			description: root.isChinese ? "胎压" : qsTr("Pressure")
			item {
				bind: root.discoveredPrefix + "/Pressure"
				unit: "bar"
				decimals: 2
			}
		}

		MbItemValue {
			description: root.isChinese ? "温度" : qsTr("Temperature")
			item {
				bind: root.discoveredPrefix + "/Temperature"
				unit: "C"
				decimals: 1
			}
		}

		MbItemOptions {
			description: root.isChinese ? "轮位" : qsTr("Wheel")
			bind: root.discoveredPrefix + "/AssignedWheel"
			possibleValues: [
				MbOption { description: root.isChinese ? "未绑定" : qsTr("Unassigned"); value: "unassigned" },
				MbOption { description: root.isChinese ? "左前" : qsTr("Front left"); value: "front_left" },
				MbOption { description: root.isChinese ? "右前" : qsTr("Front right"); value: "front_right" },
				MbOption { description: root.isChinese ? "左后" : qsTr("Rear left"); value: "rear_left" },
				MbOption { description: root.isChinese ? "右后" : qsTr("Rear right"); value: "rear_right" }
			]
		}
	}
}
