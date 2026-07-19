import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbSubMenu {
	id: root
	property int serviceIndex: 0
	property string serviceRoot: "com.victronenergy.rathole/Services/" + root.serviceIndex
	property VBusItem visibleItem: VBusItem { bind: root.serviceRoot + "/Visible" }
	property VBusItem nameItem: VBusItem { bind: root.serviceRoot + "/Name" }
	description: nameItem.value
	show: visibleItem.value === 1
	item: VBusItem { value: [] }
	MbTextBlock { item.bind: root.serviceRoot + "/Summary"; width: 190; height: 25 }
	subpage: Component { PageRatholeService { serviceIndex: root.serviceIndex } }
}
