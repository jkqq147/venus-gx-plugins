import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	property string mode: "installed"
	property string service: "com.victronenergy.pluginmanager"
	property VBusItem ids: VBusItem {
		bind: root.service + (root.mode === "installed" ? "/InstalledIds" : "/AvailableIds")
	}
	property variant pluginIds: ids.valid && ids.value !== "" ? String(ids.value).split(",") : []
	title: mode === "installed" ? qsTr("Installed plugins") : qsTr("Available plugins")

	model: VisualModels {
		VisibleItemModel {
			MbItemText {
				text: root.mode === "installed"
					? qsTr("No plugins installed")
					: qsTr("No plugins available. Use Refresh to update the catalog.")
				wrapMode: Text.WordWrap
				show: root.pluginIds.length === 0
			}
		}

		VisualDataModel {
			model: root.pluginIds

			delegate: MbSubMenu {
				id: pluginEntry
				property string pluginId: modelData
				property string pluginKey: pluginId.replace(/-/g, "_")
				property string pluginRoot: root.service + "/Plugins/" + pluginKey
				property VBusItem pluginName: VBusItem { bind: pluginEntry.pluginRoot + "/Name" }

				description: pluginName.valid ? pluginName.value : pluginId
				item.bind: pluginRoot + "/Status"
				subpage: Component {
					PagePluginDetails { pluginId: pluginEntry.pluginId }
				}
			}
		}
	}
}
