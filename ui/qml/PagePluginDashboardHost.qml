import QtQuick 1.1
import com.victron.velib 1.0

StackPage {
	id: root
	property variant dashboardComponent

	showToolBar: true
	focus: active

	Keys.onLeftPressed: pageStack.pop()
	Keys.onEscapePressed: pageStack.pop()
	Keys.onReturnPressed: pageStack.pop()

	Loader {
		anchors.fill: parent
		sourceComponent: root.dashboardComponent
		onLoaded: {
			item.visible = true
			item.width = width
			item.height = height
		}
	}
}
