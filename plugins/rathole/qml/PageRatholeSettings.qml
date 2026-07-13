import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	title: qsTr("Rathole")
	property string pluginRoot: "com.victronenergy.pluginmanager/Plugins/rathole"
	property string configPath: "/data/venus-gx-plugins/config/rathole/client.toml"

	model: VisibleItemModel {
		MbItemValue {
			description: qsTr("Service")
			item.bind: root.pluginRoot + "/ServiceState"
		}

		MbItemValue {
			description: qsTr("Configuration")
			item: VBusItem { value: root.configPath }
		}

		MbItemText {
			text: qsTr("Edit client.toml through SSH with nano. Disable and enable the plugin after saving changes.")
			wrapMode: Text.WordWrap
		}
	}
}
