import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	property string mode: "installed"
	title: mode === "installed" ? qsTr("Installed plugins") : qsTr("Available plugins")

	model: VisibleItemModel {
		MbTextBlock {
			text: mode === "installed"
				? qsTr("Plugin entries are provided by Plugin Manager.")
				: qsTr("Use Refresh to update the plugin catalog.")
			wrapMode: Text.WordWrap
			width: parent ? parent.width : 400
		}
	}
}
