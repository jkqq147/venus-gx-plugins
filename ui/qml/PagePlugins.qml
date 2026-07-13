import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	title: qsTr("Plugins")
	property string service: "com.victronenergy.pluginmanager"
	property VBusItem busy: VBusItem { bind: root.service + "/Busy" }
	property VBusItem error: VBusItem { bind: root.service + "/LastError" }
	property VBusItem refresh: VBusItem { bind: root.service + "/Refresh" }

	model: VisibleItemModel {
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
