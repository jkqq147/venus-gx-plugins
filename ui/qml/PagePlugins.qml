import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	title: "插件"
	property string service: "com.victronenergy.pluginmanager"
	property VBusItem busy: VBusItem { bind: root.service + "/Busy" }
	property VBusItem error: VBusItem { bind: root.service + "/LastError" }
	property VBusItem refresh: VBusItem { bind: root.service + "/Refresh" }
	property VBusItem catalogLoaded: VBusItem { bind: root.service + "/CatalogLoaded" }
	property VBusItem managerAvailableVersion: VBusItem { bind: root.service + "/Manager/AvailableVersion" }
	property VBusItem managerHasUpdate: VBusItem { bind: root.service + "/Manager/HasUpdate" }
	property VBusItem managerUpdate: VBusItem { bind: root.service + "/Manager/Update" }

	model: VisibleItemModel {
		MbSubMenu {
			description: "已安装插件"
			item.bind: root.service + "/InstalledCount"
			subpage: Component { PagePluginList { mode: "installed" } }
		}

		MbSubMenu {
			description: "获取插件"
			item.bind: root.service + "/AvailableCount"
			subpage: Component { PagePluginList { mode: "available" } }
		}

		MbOK {
			description: "检查更新"
			value: busy.value === 1
				? "检查中..."
				: catalogLoaded.value === 1
					? "已检查"
					: "点击检查"
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: refresh.setValue(1)
		}

		MbOK {
			description: "更新插件管理器"
			value: busy.value === 1
				? "更新中..."
				: managerAvailableVersion.valid
					? String(managerAvailableVersion.value)
					: ""
			show: managerHasUpdate.value === 1
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: managerUpdate.setValue(1)
		}

		MbItemText {
			text: error.valid ? String(error.value) : ""
			wrapMode: Text.WordWrap
			show: error.valid && error.value !== ""
		}
	}
}
