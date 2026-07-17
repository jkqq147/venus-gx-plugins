import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	title: qsTr("TPMS")
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"

	model: VisibleItemModel {
		MbSubMenu {
			description: root.isChinese ? "左前" : qsTr("Front left")
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Slots/front_left/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Slots/front_left/TemperatureDisplay"; width: 82; height: 25 }
			subpage: Component { PageTpmsWheel { wheelKey: "front_left"; pageTitle: root.isChinese ? "左前" : qsTr("Front left") } }
		}

		MbSubMenu {
			description: root.isChinese ? "右前" : qsTr("Front right")
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Slots/front_right/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Slots/front_right/TemperatureDisplay"; width: 82; height: 25 }
			subpage: Component { PageTpmsWheel { wheelKey: "front_right"; pageTitle: root.isChinese ? "右前" : qsTr("Front right") } }
		}

		MbSubMenu {
			description: root.isChinese ? "左后" : qsTr("Rear left")
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Slots/rear_left/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Slots/rear_left/TemperatureDisplay"; width: 82; height: 25 }
			subpage: Component { PageTpmsWheel { wheelKey: "rear_left"; pageTitle: root.isChinese ? "左后" : qsTr("Rear left") } }
		}

		MbSubMenu {
			description: root.isChinese ? "右后" : qsTr("Rear right")
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Slots/rear_right/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Slots/rear_right/TemperatureDisplay"; width: 82; height: 25 }
			subpage: Component { PageTpmsWheel { wheelKey: "rear_right"; pageTitle: root.isChinese ? "右后" : qsTr("Rear right") } }
		}

		MbSubMenu {
			description: root.isChinese ? "扫描传感器" : qsTr("Discover sensors")
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/DiscoveredCount"; width: 92; height: 25 }
			subpage: Component { PageTpmsDiscovered {} }
		}

		MbSubMenu {
			description: root.isChinese ? "诊断" : qsTr("Diagnostics")
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/StatusText"; width: 110; height: 25 }
			subpage: Component { PageTpmsDiagnostics {} }
		}
	}
}
