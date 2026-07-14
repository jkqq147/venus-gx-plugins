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
	property VBusItem pluginDescription: VBusItem { bind: root.pluginRoot + "/Description" }
	property VBusItem installed: VBusItem { bind: root.pluginRoot + "/Installed" }
	property VBusItem available: VBusItem { bind: root.pluginRoot + "/Available" }
	property VBusItem enabledItem: VBusItem { bind: root.pluginRoot + "/Enabled" }
	property VBusItem hasUpdate: VBusItem { bind: root.pluginRoot + "/HasUpdate" }
	property VBusItem catalogVersion: VBusItem { bind: root.pluginRoot + "/CatalogVersion" }
	property VBusItem hasSettingsPage: VBusItem { bind: root.pluginRoot + "/HasSettingsPage" }
	property VBusItem settingsPage: VBusItem { bind: root.pluginRoot + "/SettingsPage" }
	property VBusItem installCommand: VBusItem { bind: root.pluginRoot + "/Install" }
	property VBusItem uninstallCommand: VBusItem { bind: root.pluginRoot + "/Uninstall" }
	property VBusItem purgeCommand: VBusItem { bind: root.pluginRoot + "/Purge" }
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
			pageLoadError = "无法打开插件页面"
			return
		}
		pageStack.push(page)
	}

	model: VisibleItemModel {
		MbItemText {
			text: pluginDescription.valid ? String(pluginDescription.value) : ""
			wrapMode: Text.WordWrap
			show: pluginDescription.valid && pluginDescription.value !== ""
		}

		MbItemValue {
			description: "版本"
			item.bind: root.pluginRoot + "/InstalledVersion"
			show: installed.value === 1
		}

		MbItemValue {
			description: "版本"
			item.bind: root.pluginRoot + "/CatalogVersion"
			show: installed.value !== 1 && available.value === 1
		}

		MbOK {
			description: installed.value === 1 ? "更新" : "安装"
			value: busy.value === 1
				? "处理中..."
				: installed.value === 1
					? "版本 " + String(root.catalogVersion.value)
					: "点击安装"
			show: available.value === 1 && (installed.value !== 1 || hasUpdate.value === 1)
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: installCommand.setValue(1)
		}

		MbSwitch {
			name: "启用"
			bind: root.pluginRoot + "/Enabled"
			show: installed.value === 1
			enabled: busy.value !== 1
		}

		MbOK {
			description: "插件设置"
			value: "打开"
			show: installed.value === 1 && root.enabledItem.value === 1 && root.hasSettingsPage.value === 1
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: root.openPluginPage(settingsPage.value)
		}

		MbOK {
			description: "卸载"
			value: "点击选择"
			show: installed.value === 1
			editable: busy.value !== 1
			enabled: busy.value !== 1
			onClicked: uninstallConfirmation.edit()
		}

		MbItemOptions {
			id: uninstallConfirmation
			description: "确认卸载"
			message: "请选择是否保留插件配置。"
			show: false
			possibleValues: [
				MbOption { description: "取消"; value: 0 },
				MbOption { description: "卸载并保留配置"; value: 1 },
				MbOption { description: "彻底删除"; value: 2 }
			]
			onOptionSelected: {
				if (newValue === 1)
					uninstallCommand.setValue(1)
				else if (newValue === 2)
					purgeConfirmation.edit()
			}
		}

		MbItemOptions {
			id: purgeConfirmation
			description: "确认彻底删除"
			message: "删除插件及其全部配置？此操作无法撤销。"
			show: false
			possibleValues: [
				MbOption { description: "取消"; value: 0 },
				MbOption { description: "永久删除"; value: 1 }
			]
			onOptionSelected: {
				if (newValue === 1)
					purgeCommand.setValue(1)
			}
		}

		MbItemText {
			text: root.pageLoadError
			wrapMode: Text.WordWrap
			show: root.pageLoadError !== ""
		}

		MbItemText {
			property VBusItem pluginError: VBusItem { bind: root.pluginRoot + "/Error" }
			text: pluginError.valid ? String(pluginError.value) : ""
			wrapMode: Text.WordWrap
			show: pluginError.valid && pluginError.value !== ""
		}
	}
}
