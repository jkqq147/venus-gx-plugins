import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	property string pluginId: ""
	property string pluginKey: pluginId.replace(/-/g, "_")
	property string service: "com.victronenergy.pluginmanager"
	property string pluginRoot: service + "/Plugins/" + pluginKey
	property string pageLoadError: ""

	property VBusItem pluginName: VBusItem { bind: root.pluginRoot + "/Name" }
	property VBusItem installed: VBusItem { bind: root.pluginRoot + "/Installed" }
	property VBusItem available: VBusItem { bind: root.pluginRoot + "/Available" }
	property VBusItem enabledItem: VBusItem { bind: root.pluginRoot + "/Enabled" }
	property VBusItem hasUpdate: VBusItem { bind: root.pluginRoot + "/HasUpdate" }
	property VBusItem hasSettingsPage: VBusItem { bind: root.pluginRoot + "/HasSettingsPage" }
	property VBusItem settingsPage: VBusItem { bind: root.pluginRoot + "/SettingsPage" }
	property VBusItem installCommand: VBusItem { bind: root.pluginRoot + "/Install" }
	property VBusItem uninstallCommand: VBusItem { bind: root.pluginRoot + "/Uninstall" }
	property VBusItem busy: VBusItem { bind: root.service + "/Busy" }

	title: pluginName.valid ? pluginName.value : pluginId

	function openPluginPage(path) {
		pageLoadError = ""
		var component = Qt.createComponent(path)
		if (component.status !== Component.Ready) {
			pageLoadError = component.errorString()
			return
		}
		var page = component.createObject(root)
		if (page === null) {
			pageLoadError = qsTr("Unable to open plugin page")
			return
		}
		pageStack.push(page)
	}

	model: VisibleItemModel {
		MbItemValue {
			description: qsTr("Status")
			item.bind: root.pluginRoot + "/Status"
		}

		MbItemValue {
			description: qsTr("Installed version")
			item.bind: root.pluginRoot + "/InstalledVersion"
			show: installed.value === 1
		}

		MbItemValue {
			description: qsTr("Available version")
			item.bind: root.pluginRoot + "/CatalogVersion"
			show: available.value === 1
		}

		MbOK {
			description: hasUpdate.value === 1 ? qsTr("Update") : qsTr("Install")
			value: busy.value === 1 ? qsTr("Working...") : qsTr("Press to continue")
			show: available.value === 1 && (installed.value !== 1 || hasUpdate.value === 1)
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: installCommand.setValue(1)
		}

		MbSwitch {
			name: qsTr("Enabled")
			bind: root.pluginRoot + "/Enabled"
			show: installed.value === 1
			enabled: busy.value !== 1
		}

		MbOK {
			description: qsTr("Plugin page")
			value: qsTr("Open")
			show: installed.value === 1 && root.enabledItem.value === 1 && root.hasSettingsPage.value === 1
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: root.openPluginPage(settingsPage.value)
		}

		MbOK {
			description: qsTr("Uninstall")
			value: qsTr("Press to continue")
			show: installed.value === 1
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: uninstallConfirmation.edit()
		}

		MbItemOptions {
			id: uninstallConfirmation
			description: qsTr("Confirm uninstall")
			message: qsTr("Uninstall this plugin? Its configuration will be kept.")
			show: false
			possibleValues: [
				MbOption { description: qsTr("Cancel"); value: 0 },
				MbOption { description: qsTr("Uninstall"); value: 1 }
			]
			onOptionSelected: {
				if (newValue === 1)
					uninstallCommand.setValue(1)
			}
		}

		MbItemText {
			text: root.pageLoadError
			wrapMode: Text.WordWrap
			show: root.pageLoadError !== ""
		}

		MbItemText {
			property VBusItem pluginError: VBusItem { bind: root.pluginRoot + "/Error" }
			text: pluginError.value
			wrapMode: Text.WordWrap
			show: pluginError.valid && pluginError.value !== ""
		}
	}
}
