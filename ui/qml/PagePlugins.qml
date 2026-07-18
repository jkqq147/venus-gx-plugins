import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	title: qsTr("Plugins")
	property string service: "com.victronenergy.pluginmanager"
	property VBusItem busy: VBusItem { bind: root.service + "/Busy" }
	property VBusItem error: VBusItem { bind: root.service + "/LastError" }
	property VBusItem refresh: VBusItem { bind: root.service + "/Refresh" }
	property VBusItem catalogLoaded: VBusItem { bind: root.service + "/CatalogLoaded" }
	property VBusItem managerAvailableVersion: VBusItem { bind: root.service + "/Manager/AvailableVersion" }
	property VBusItem managerHasUpdate: VBusItem { bind: root.service + "/Manager/HasUpdate" }
	property VBusItem managerUpdate: VBusItem { bind: root.service + "/Manager/Update" }

	model: VisibleItemModel {
		MbSubMenu {
			description: qsTr("Installed plugins")
			item.bind: root.service + "/InstalledCount"
			subpage: Component { PagePluginList { mode: "installed" } }
		}

		MbSubMenu {
			description: qsTr("Get plugins")
			item.bind: root.service + "/AvailableCount"
			subpage: Component { PagePluginList { mode: "available" } }
		}

		MbOK {
			description: qsTr("Check for updates")
			value: busy.value === 1
				? qsTr("Checking...")
				: catalogLoaded.value === 1
					? qsTr("Checked")
					: qsTr("Press to check")
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: refresh.setValue(1)
		}

		MbOK {
			description: qsTr("Update Plugin Manager")
			value: busy.value === 1 ? qsTr("Updating...") : managerAvailableVersion.value
			show: managerHasUpdate.value === 1
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: managerUpdate.setValue(1)
		}

		MbItemText {
			text: error.valid ? String(error.value) : ""
			wrapMode: Text.WordWrap
			show: error.valid && error.value !== ""
		}

		MbItemValue {
			description: qsTr("Version")
			item.bind: root.service + "/Manager/InstalledVersion"
		}
	}
}
