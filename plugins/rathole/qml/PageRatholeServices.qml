import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	property string serviceRoot: "com.victronenergy.rathole"
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	property VBusItem serviceCount: VBusItem { bind: root.serviceRoot + "/Services/Count" }
	title: root.isChinese ? "转发服务" : qsTr("Forwarded services")

	model: VisibleItemModel {
		RatholeServiceRow { serviceIndex: 0 }
		RatholeServiceRow { serviceIndex: 1 }
		RatholeServiceRow { serviceIndex: 2 }
		RatholeServiceRow { serviceIndex: 3 }
		RatholeServiceRow { serviceIndex: 4 }
		RatholeServiceRow { serviceIndex: 5 }
		RatholeServiceRow { serviceIndex: 6 }
		RatholeServiceRow { serviceIndex: 7 }

		MbSubMenu {
			description: root.isChinese ? "添加服务" : qsTr("Add service")
			show: root.serviceCount.value < 8
			item: VBusItem { value: [] }
			subpage: Component { PageRatholeAddService {} }
		}
	}
}
