import QtQuick 1.1
import com.victron.velib 1.0

MbPage {
	id: root
	property string mode: "installed"
	property string service: "com.victronenergy.pluginmanager"
	property VBusItem ids: VBusItem {
		bind: root.service + (root.mode === "installed" ? "/InstalledIds" : "/AvailableIds")
	}
	property variant pluginIds: ids.valid && ids.value !== "" ? String(ids.value).split(",") : []
	title: mode === "installed" ? "已安装插件" : "获取插件"

	model: VisualModels {
		VisibleItemModel {
			MbItemText {
				text: root.mode === "installed"
					? "尚未安装插件"
					: "暂无插件信息，请返回检查更新。"
				wrapMode: Text.WordWrap
				show: root.pluginIds.length === 0
			}
		}

		VisualDataModel {
			model: root.pluginIds

			delegate: MbSubMenu {
				id: pluginEntry
				property string pluginId: modelData
				property string pluginKey: pluginId.replace(/-/g, "_")
				property string pluginRoot: root.service + "/Plugins/" + pluginKey
				property VBusItem pluginName: VBusItem { bind: pluginEntry.pluginRoot + "/Name" }
				property VBusItem pluginDescription: VBusItem { bind: pluginEntry.pluginRoot + "/Description" }
				property VBusItem lifecycle: VBusItem { bind: pluginEntry.pluginRoot + "/Lifecycle" }
				property VBusItem hasUpdate: VBusItem { bind: pluginEntry.pluginRoot + "/HasUpdate" }
				property string summary: root.mode === "available"
					? (pluginDescription.valid ? String(pluginDescription.value) : "")
					: hasUpdate.value === 1
						? "有可用更新"
						: lifecycle.value === "enabled"
							? "已开启"
							: lifecycle.value === "degraded"
								? "需要处理"
								: "已关闭"

				description: pluginName.valid ? pluginName.value : pluginId
				item: VBusItem { value: pluginEntry.summary }
				subpage: Component {
					PagePluginDetails { pluginId: pluginEntry.pluginId }
				}
			}
		}
	}
}
