import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	title: pageTitle

	property string wheelKey: ""
	property string pageTitle: qsTr("TPMS")
	property string slotPrefix: "com.victronenergy.tpms.main/Slots/" + wheelKey
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property VBusItem stateItem: VBusItem { bind: root.slotPrefix + "/State" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"

	function stateText(value) {
		if (!root.isChinese)
			return value === "ok" ? "OK" : value === "stale" ? "Stale" : value === "waiting" ? "Waiting" : "Unassigned"
		return value === "ok" ? "正常" : value === "stale" ? "已过期" : value === "waiting" ? "等待数据" : "未绑定"
	}

	model: VisibleItemModel {
		MbItemValue {
			description: root.isChinese ? "胎压" : qsTr("Pressure")
			item {
				bind: root.slotPrefix + "/Pressure"
				unit: "bar"
				decimals: 2
			}
		}

		MbItemValue {
			description: root.isChinese ? "温度" : qsTr("Temperature")
			item {
				bind: root.slotPrefix + "/Temperature"
				unit: "C"
				decimals: 1
			}
		}

		MbItemValue {
			description: root.isChinese ? "状态" : qsTr("Status")
			item: VBusItem { value: root.stateText(root.stateItem.value) }
		}

		MbSubMenu {
			description: root.isChinese ? "传感器详情" : qsTr("Sensor details")
			item: VBusItem { value: [] }
			subpage: Component { PageTpmsSensorDetails { slotPrefix: root.slotPrefix } }
		}
	}
}
