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
	title: mode === "installed" ? qsTr("Installed plugins") : qsTr("Get plugins")

	model: VisualModels {
		VisibleItemModel {
			MbItemText {
				text: root.mode === "installed"
					? qsTr("No plugins installed")
					: qsTr("No plugin information. Go back and check for updates.")
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
				property VBusItem lifecycle: VBusItem { bind: pluginEntry.pluginRoot + "/Lifecycle" }
				property VBusItem hasUpdate: VBusItem { bind: pluginEntry.pluginRoot + "/HasUpdate" }
				property string summary: root.mode === "installed"
					? hasUpdate.value === 1
						? qsTr("Update available")
						: lifecycle.value === "enabled"
							? qsTr("On")
							: lifecycle.value === "degraded"
								? qsTr("Needs attention")
								: qsTr("Off")
					: ""

				description: pluginName.valid ? pluginName.value : pluginId
				item: VBusItem { value: pluginEntry.summary }
				subpage: Component {
					PagePluginDetails { pluginId: pluginEntry.pluginId }
				}
			}
		}
	}
}
