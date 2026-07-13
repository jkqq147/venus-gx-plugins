import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	title: qsTr("Plugins")

	model: VisibleItemModel {
		MbSubMenu {
			description: qsTr("Installed plugins")
			item: VBusItem { value: [] }
			MbTextBlock {
				item.bind: "com.victronenergy.pluginmanager/InstalledCount"
				width: 90
				height: 25
			}
			subpage: Component { PagePluginList { mode: "installed" } }
		}

		MbSubMenu {
			description: qsTr("Available plugins")
			item: VBusItem { value: [] }
			MbTextBlock {
				item.bind: "com.victronenergy.pluginmanager/CatalogStatus"
				width: 150
				height: 25
			}
			subpage: Component { PagePluginList { mode: "available" } }
		}

		MbItemOptions {
			description: qsTr("Refresh")
			bind: "com.victronenergy.pluginmanager/Refresh"
			possibleValues: [0, 1]
			showOptions: false
		}
	}
}
