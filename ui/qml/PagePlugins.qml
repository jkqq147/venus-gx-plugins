import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	title: qsTr("Plugins")
	property string service: "com.victronenergy.pluginmanager"
	property VBusItem busy: VBusItem { bind: root.service + "/Busy" }
	property VBusItem error: VBusItem { bind: root.service + "/LastError" }
	property VBusItem refresh: VBusItem { bind: root.service + "/Refresh" }
	property VBusItem managerAvailableVersion: VBusItem { bind: root.service + "/Manager/AvailableVersion" }
	property VBusItem managerHasUpdate: VBusItem { bind: root.service + "/Manager/HasUpdate" }
	property VBusItem managerUpdate: VBusItem { bind: root.service + "/Manager/Update" }

	model: VisibleItemModel {
		MbItemValue {
			description: qsTr("Plugin Manager")
			item.bind: root.service + "/Manager/InstalledVersion"
		}

		MbOK {
			description: qsTr("Update Plugin Manager")
			value: busy.value === 1 ? qsTr("Working...") : managerAvailableVersion.value
			show: managerHasUpdate.value === 1
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: managerUpdate.setValue(1)
		}

		MbSubMenu {
			description: qsTr("Installed plugins")
			item.bind: root.service + "/InstalledCount"
			subpage: Component { PagePluginList { mode: "installed" } }
		}

		MbSubMenu {
			description: qsTr("Available plugins")
			item.bind: root.service + "/AvailableCount"
			subpage: Component { PagePluginList { mode: "available" } }
		}

		MbItemValue {
			description: qsTr("Catalog")
			item.bind: root.service + "/CatalogStatus"
		}

		MbOK {
			description: qsTr("Refresh")
			value: busy.value === 1 ? qsTr("Working...") : qsTr("Press to refresh")
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: refresh.setValue(1)
		}

		MbItemText {
			text: error.valid ? String(error.value) : ""
			wrapMode: Text.WordWrap
			show: error.valid && error.value !== ""
		}
	}
}
