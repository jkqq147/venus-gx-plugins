import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

MbPage {
	id: root
	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property VBusItem statusItem: VBusItem { bind: "com.victronenergy.tpms.main/StatusText" }
	property VBusItem bluetoothStatusItem: VBusItem { bind: "com.victronenergy.tpms.main/Bluetooth/StatusText" }
	property VBusItem receiverItem: VBusItem { bind: "com.victronenergy.tpms.main/Bluetooth/ReceiverStatus" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	title: root.isChinese ? "诊断" : qsTr("Diagnostics")

	function statusText(value) {
		if (!root.isChinese)
			return value
		return value === "Scanning" ? "扫描中" : value === "No Bluetooth adapter" ? "未检测到蓝牙适配器" : value === "Starting" ? "启动中" : value
	}

	function receiverText(value) {
		if (!root.isChinese)
			return value
		return value === "Receiving" ? "接收中" : value === "Listening" ? "等待广播" : value === "Unavailable" ? "不可用" : value === "Starting" ? "启动中" : value
	}

	model: VisibleItemModel {
		MbItemValue {
			description: root.isChinese ? "状态" : qsTr("Status")
			item: VBusItem { value: root.statusText(root.statusItem.value) }
		}

		MbItemValue {
			description: qsTr("Bluetooth")
			item: VBusItem { value: root.statusText(root.bluetoothStatusItem.value) }
		}

		MbItemValue {
			description: root.isChinese ? "BLE 接收" : qsTr("BLE receiver")
			item: VBusItem { value: root.receiverText(root.receiverItem.value) }
		}

		MbItemValue {
			description: root.isChinese ? "BLE 活动（60 秒）" : qsTr("BLE activity (60 sec)")
			item.bind: "com.victronenergy.tpms.main/Bluetooth/DeviceCount"
		}

		MbItemValue {
			description: root.isChinese ? "厂商数据（60 秒）" : qsTr("Manufacturer data (60 sec)")
			item.bind: "com.victronenergy.tpms.main/Bluetooth/ManufacturerDataCount"
		}
	}
}
