import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	property string serviceRoot: "com.victronenergy.loxonetanks"
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	property VBusItem selectCommand: VBusItem { bind: root.serviceRoot + "/Discovery/Select" }

	title: root.isChinese ? "选择 Miniserver" : qsTr("Select Miniserver")

	function choose(index) {
		root.selectCommand.setValue(index)
		pageStack.pop()
	}

	model: VisibleItemModel {
		MbOK {
			property VBusItem address: VBusItem { bind: root.serviceRoot + "/Discovery/Results/0/Address" }
			property VBusItem version: VBusItem { bind: root.serviceRoot + "/Discovery/Results/0/Version" }
			description: address.value
			value: version.value === "" ? "" : "v" + version.value
			show: address.valid && address.value !== ""
			onClicked: root.choose(0)
		}

		MbOK {
			property VBusItem address: VBusItem { bind: root.serviceRoot + "/Discovery/Results/1/Address" }
			property VBusItem version: VBusItem { bind: root.serviceRoot + "/Discovery/Results/1/Version" }
			description: address.value
			value: version.value === "" ? "" : "v" + version.value
			show: address.valid && address.value !== ""
			onClicked: root.choose(1)
		}

		MbOK {
			property VBusItem address: VBusItem { bind: root.serviceRoot + "/Discovery/Results/2/Address" }
			property VBusItem version: VBusItem { bind: root.serviceRoot + "/Discovery/Results/2/Version" }
			description: address.value
			value: version.value === "" ? "" : "v" + version.value
			show: address.valid && address.value !== ""
			onClicked: root.choose(2)
		}

		MbOK {
			property VBusItem address: VBusItem { bind: root.serviceRoot + "/Discovery/Results/3/Address" }
			property VBusItem version: VBusItem { bind: root.serviceRoot + "/Discovery/Results/3/Version" }
			description: address.value
			value: version.value === "" ? "" : "v" + version.value
			show: address.valid && address.value !== ""
			onClicked: root.choose(3)
		}
	}
}
