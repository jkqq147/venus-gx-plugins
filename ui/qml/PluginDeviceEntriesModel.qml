import QtQuick 1.1
import com.victron.velib 1.0

VisualDataModel {
	id: root
	property variant view
	property bool completed
	property VBusItem entryIds: VBusItem {
		bind: "com.victronenergy.pluginmanager/DeviceEntryIds"
	}
	property string observedEntryIds: entryIds.valid ? String(entryIds.value) : ""
	model: entryIds.valid && entryIds.value !== "" ? String(entryIds.value).split(",") : []

	// Qt 4.8 can retain stale indexes in the outer VisualModels when this
	// delegate model changes. Remount it on the next event-loop turn.
	property variant relayoutTimer: Timer {
		interval: 0
		onTriggered: root.rebuildView()
	}

	Component.onCompleted: completed = true

	onObservedEntryIdsChanged: {
		if (completed)
			relayoutTimer.restart()
	}

	function rebuildView() {
		if (!view || !view.model)
			return

		var aggregateModel = view.model
		view.model = null
		view.model = aggregateModel
		if (view.count === 0)
			return
		view.currentIndex = 0
		view.positionViewAtIndex(0, ListView.Beginning)
	}

	delegate: MbSubMenu {
		id: pluginEntry
		property string pluginId: modelData
		property string pluginKey: pluginId.replace(/-/g, "_")
		property string pluginRoot: "com.victronenergy.pluginmanager/Plugins/" + pluginKey
		property VBusItem pluginName: VBusItem { bind: pluginEntry.pluginRoot + "/Name" }
		property VBusItem settingsPage: VBusItem { bind: pluginEntry.pluginRoot + "/SettingsPage" }
		property VBusItem deviceListValues: VBusItem {
			bind: pluginEntry.pluginRoot + "/DeviceListValues"
		}
		property variant valuePaths: deviceListValues.valid && deviceListValues.value !== ""
			? String(deviceListValues.value).split("\n")
			: []
		property variant settingsComponent: settingsPage.valid && settingsPage.value !== ""
			? Qt.createComponent(settingsPage.value)
			: undefined

		description: pluginName.valid && pluginName.value !== ""
			? String(pluginName.value)
			: pluginId
		item: VBusItem { value: [] }
		subpage: settingsComponent !== undefined && settingsComponent.status === Component.Ready
			? settingsComponent
			: undefined

		Repeater {
			model: pluginEntry.valuePaths
			MbTextBlock {
				item.bind: modelData
				width: 62
				height: 25
			}
		}
	}
}
