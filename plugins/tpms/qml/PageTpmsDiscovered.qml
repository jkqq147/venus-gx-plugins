import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	title: root.isChinese ? "扫描传感器" : qsTr("Discover sensors")

	model: VisibleItemModel {
		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/0/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/0/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/0/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/0/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 0 } }
		}

		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/1/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/1/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/1/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/1/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 1 } }
		}

		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/2/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/2/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/2/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/2/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 2 } }
		}

		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/3/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/3/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/3/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/3/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 3 } }
		}

		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/4/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/4/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/4/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/4/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 4 } }
		}

		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/5/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/5/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/5/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/5/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 5 } }
		}

		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/6/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/6/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/6/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/6/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 6 } }
		}

		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/7/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/7/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/7/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/7/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 7 } }
		}

		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/8/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/8/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/8/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/8/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 8 } }
		}

		MbSubMenu {
			property VBusItem name: VBusItem { bind: "com.victronenergy.tpms.main/Discovered/9/Name" }
			description: name.value
			item: VBusItem { value: [] }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/9/PressureDisplay"; width: 92; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/9/TemperatureDisplay"; width: 82; height: 25 }
			MbTextBlock { item.bind: "com.victronenergy.tpms.main/Discovered/9/RssiDisplay"; width: 66; height: 25 }
			show: name.valid && name.value !== ""
			subpage: Component { PageTpmsBind { sensorIndex: 9 } }
		}
	}
}
