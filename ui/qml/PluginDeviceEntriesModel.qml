import QtQuick 1.1
import com.victron.velib 1.0

VisualDataModel {
	id: root
	property string service: "com.victronenergy.pluginmanager"
	property VBusItem entryIds: VBusItem { bind: root.service + "/DeviceEntryIds" }
	model: entryIds.valid && entryIds.value !== "" ? String(entryIds.value).split(",") : []

	delegate: MbSubMenu {
		id: pluginEntry
		property string pluginId: modelData
		property string pluginKey: pluginId.replace(/-/g, "_")
		property string pluginRoot: root.service + "/Plugins/" + pluginKey
		property VBusItem pluginName: VBusItem { bind: pluginEntry.pluginRoot + "/Name" }
		property VBusItem settingsPage: VBusItem { bind: pluginEntry.pluginRoot + "/SettingsPage" }
		property variant settingsComponent: settingsPage.valid && settingsPage.value !== ""
			? Qt.createComponent(settingsPage.value)
			: undefined

		description: pluginName.valid ? pluginName.value : pluginId
		item.bind: pluginRoot + "/Status"
		subpage: settingsComponent !== undefined && settingsComponent.status === Component.Ready
			? settingsComponent
			: undefined
	}
}
