import QtQuick 1.1
import com.victron.velib 1.0

Item {
	id: root
	property variant overviewModel
	property string pluginRoot: "/data/venus-gx-plugins/state/plugins/"
	signal addDashboard(string source)

	property VBusItem guiReady: VBusItem {
		bind: "com.victronenergy.pluginmanager/Gui/Ready"
		onValidChanged: {
			if (valid)
				setValue(1)
		}
	}
	property VBusItem dashboardSources: VBusItem {
		bind: "com.victronenergy.pluginmanager/DashboardSources"
		onValueChanged: syncTimer.restart()
		onValidChanged: syncTimer.restart()
	}

	Timer {
		id: syncTimer
		interval: 1
		repeat: false
		onTriggered: root.syncDashboards()
	}

	Component.onCompleted: {
		guiReady.setValue(1)
		syncTimer.restart()
	}

	function isPluginDashboard(source) {
		return source.indexOf(root.pluginRoot) === 0 && source.slice(-4) === ".qml"
	}

	function syncDashboards() {
		if (root.overviewModel === undefined)
			return

		for (var i = root.overviewModel.count - 1; i >= 0; --i) {
			if (isPluginDashboard(String(root.overviewModel.get(i).pageSource)))
				root.overviewModel.remove(i)
		}

		if (!dashboardSources.valid || dashboardSources.value === "")
			return

		var sources = String(dashboardSources.value).split("\n")
		for (i = 0; i < sources.length; ++i) {
			var source = String(sources[i])
			if (isPluginDashboard(source))
				root.addDashboard(source)
		}
	}
}
