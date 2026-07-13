import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	property string service: "com.victronenergy.pluginmanager"
	property VBusItem ids: VBusItem { bind: root.service + "/DashboardIds" }
	property variant pluginIds: ids.valid && ids.value !== "" ? String(ids.value).split(",") : []
	property string pageLoadError: ""
	title: qsTr("Plugin dashboards")

	function openDashboard(path) {
		pageLoadError = ""
		var component = Qt.createComponent(path)
		if (component.status !== Component.Ready) {
			pageLoadError = component.errorString()
			return
		}
		var page = component.createObject(root)
		if (page === null) {
			pageLoadError = qsTr("Unable to open dashboard")
			return
		}
		pageStack.push(page)
	}

	model: VisualModels {
		VisibleItemModel {
			MbItemText {
				text: root.pageLoadError
				wrapMode: Text.WordWrap
				show: root.pageLoadError !== ""
			}
		}

		VisualDataModel {
			model: root.pluginIds

			delegate: MbOK {
				id: dashboardEntry
				property string pluginId: modelData
				property string pluginKey: pluginId.replace(/-/g, "_")
				property string pluginRoot: root.service + "/Plugins/" + pluginKey
				property VBusItem pluginName: VBusItem { bind: dashboardEntry.pluginRoot + "/Name" }
				property VBusItem componentPath: VBusItem { bind: dashboardEntry.pluginRoot + "/DashboardComponent" }

				description: pluginName.valid ? pluginName.value : pluginId
				value: qsTr("Open")
				editable: componentPath.valid && componentPath.value !== ""
				enabled: componentPath.valid && componentPath.value !== ""
				onClicked: root.openDashboard(componentPath.value)
			}
		}
	}
}
