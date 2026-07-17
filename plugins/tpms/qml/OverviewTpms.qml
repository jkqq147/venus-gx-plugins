import QtQuick 1.1
import com.victron.velib 1.0
import "/opt/victronenergy/gui/qml"

OverviewPage {
	id: root
	title: qsTr("TPMS")

	property VBusItem guiLanguage: VBusItem { bind: "com.victronenergy.settings/Settings/Gui/Language" }
	property bool isChinese: guiLanguage.valid && guiLanguage.value === "zh"
	property int clockTick: 0

	Timer {
		interval: 60000
		running: true
		repeat: true
		onTriggered: root.clockTick++
	}

	function wheelLabel(key) {
		if (root.isChinese)
			return key === "front_left" ? "左前" : key === "front_right" ? "右前" : key === "rear_left" ? "左后" : "右后"
		return key === "front_left" ? qsTr("Front left") : key === "front_right" ? qsTr("Front right") : key === "rear_left" ? qsTr("Rear left") : qsTr("Rear right")
	}

	function stateLabel(state) {
		if (!root.isChinese)
			return state === "stale" ? qsTr("Stale") : state === "waiting" ? qsTr("Waiting") : qsTr("Unassigned")
		return state === "stale" ? "已过期" : state === "waiting" ? "等待数据" : "未绑定"
	}

	function stateColor(state) {
		return state === "ok" ? "#3ECF8E" : state === "stale" ? "#F5A623" : "#8C9AA8"
	}

	function lastUpdateText(lastSeen) {
		root.clockTick
		if (lastSeen === undefined || lastSeen === null || lastSeen <= 0)
			return root.isChinese ? "更新已过期" : qsTr("Update stale")
		var age = Math.max(0, Math.floor(new Date().getTime() / 1000) - Number(lastSeen))
		if (root.isChinese) {
			if (age < 90)
				return "刚刚更新"
			if (age < 3600)
				return Math.floor(age / 60) + " 分钟前"
			if (age < 86400)
				return Math.floor(age / 3600) + " 小时前"
			return Math.floor(age / 86400) + " 天前"
		}
		if (age < 90)
			return qsTr("Updated now")
		if (age < 3600)
			return Math.floor(age / 60) + qsTr(" min ago")
		if (age < 86400)
			return Math.floor(age / 3600) + qsTr(" h ago")
		return Math.floor(age / 86400) + qsTr(" d ago")
	}

	Component {
		id: wheelTile

		Item {
			id: tile
			property string wheelKey: ""
			property bool leftSide: true
			property string slotPrefix: "com.victronenergy.tpms.main/Slots/" + wheelKey
			property VBusItem pressure: VBusItem { bind: tile.slotPrefix + "/PressureDisplay" }
			property VBusItem temperature: VBusItem { bind: tile.slotPrefix + "/TemperatureDisplay" }
			property VBusItem state: VBusItem { bind: tile.slotPrefix + "/State" }
			property VBusItem lastSeen: VBusItem { bind: tile.slotPrefix + "/LastSeen" }
			property bool hasReading: state.value === "ok" || state.value === "stale"
			property int contentX: leftSide ? 0 : 16

			width: 124
			height: 72

			Rectangle {
				width: 6
				height: 6
				radius: 3
				x: tile.contentX
				y: 1
				color: root.stateColor(tile.state.value)
			}

			Text {
				x: tile.contentX + 10
				y: 0
				text: root.wheelLabel(tile.wheelKey)
				color: "#718093"
				font.pixelSize: 12
			}

			Text {
				id: pressureValue
				x: tile.contentX
				y: 16
				text: tile.hasReading && tile.pressure.valid ? tile.pressure.value : "--"
				color: tile.hasReading ? "#263746" : "#9DABB7"
				font.pixelSize: 28
				font.bold: true
			}

			Text {
				x: pressureValue.x + pressureValue.paintedWidth + 3
				y: pressureValue.y + 15
				text: tile.hasReading ? "bar" : ""
				color: "#718093"
				font.pixelSize: 12
			}

			Text {
				id: temperatureValue
				x: tile.contentX
				y: 57
				text: tile.hasReading && tile.temperature.valid ? tile.temperature.value : root.stateLabel(tile.state.value)
				color: tile.hasReading ? "#718093" : root.stateColor(tile.state.value)
				font.pixelSize: 12
			}

			Text {
				x: temperatureValue.x + temperatureValue.paintedWidth + 7
				y: temperatureValue.y
				visible: tile.hasReading && tile.state.value === "stale"
				text: root.lastUpdateText(tile.lastSeen.value)
				color: root.stateColor(tile.state.value)
				font.pixelSize: 12
			}
		}
	}

	Item {
		id: vehicle
		width: 214
		height: 190
		anchors.centerIn: parent
		property int bodyLeft: 69
		property int bodyRight: 145
		property int rightWheelX: 204
		property int frontAxleY: 36
		property int rearAxleY: 154
		property int axleGap: 4

		Rectangle {
			x: 10
			y: vehicle.frontAxleY
			width: vehicle.bodyLeft - vehicle.axleGap - 10
			height: 1
			color: "#C6D2DA"
		}

		Rectangle {
			x: vehicle.bodyRight + vehicle.axleGap
			y: vehicle.frontAxleY
			width: vehicle.rightWheelX - vehicle.bodyRight - vehicle.axleGap
			height: 1
			color: "#C6D2DA"
		}

		Rectangle {
			x: 10
			y: vehicle.rearAxleY
			width: vehicle.bodyLeft - vehicle.axleGap - 10
			height: 1
			color: "#C6D2DA"
		}

		Rectangle {
			x: vehicle.bodyRight + vehicle.axleGap
			y: vehicle.rearAxleY
			width: vehicle.rightWheelX - vehicle.bodyRight - vehicle.axleGap
			height: 1
			color: "#C6D2DA"
		}

		Rectangle {
			width: 76
			height: 168
			anchors.centerIn: parent
			radius: 19
			color: "transparent"
			border.width: 2
			border.color: "#617887"
		}

		Rectangle {
			width: 52
			height: 50
			anchors { horizontalCenter: parent.horizontalCenter; top: parent.top; topMargin: 43 }
			radius: 10
			color: "#EEF3F6"
			border.width: 1
			border.color: "#B8C7D0"
		}

		Rectangle {
			width: 46
			height: 24
			anchors { horizontalCenter: parent.horizontalCenter; bottom: parent.bottom; bottomMargin: 30 }
			radius: 7
			color: "transparent"
			border.width: 1
			border.color: "#B8C7D0"
		}

		Rectangle {
			x: 0
			y: vehicle.frontAxleY - height / 2
			width: 10
			height: 44
			radius: 4
			color: "#26333D"
			border.width: 1
			border.color: "#4A5A67"
		}

		Rectangle {
			x: vehicle.rightWheelX
			y: vehicle.frontAxleY - height / 2
			width: 10
			height: 44
			radius: 4
			color: "#26333D"
			border.width: 1
			border.color: "#4A5A67"
		}

		Rectangle {
			x: 0
			y: vehicle.rearAxleY - height / 2
			width: 10
			height: 44
			radius: 4
			color: "#26333D"
			border.width: 1
			border.color: "#4A5A67"
		}

		Rectangle {
			x: vehicle.rightWheelX
			y: vehicle.rearAxleY - height / 2
			width: 10
			height: 44
			radius: 4
			color: "#26333D"
			border.width: 1
			border.color: "#4A5A67"
		}
	}

	Loader {
		anchors { left: parent.left; leftMargin: 14; top: parent.top; topMargin: 20 }
		width: 124
		height: 72
		sourceComponent: wheelTile
		onLoaded: { item.wheelKey = "front_left"; item.leftSide = true }
	}

	Loader {
		anchors { right: parent.right; rightMargin: 14; top: parent.top; topMargin: 20 }
		width: 124
		height: 72
		sourceComponent: wheelTile
		onLoaded: { item.wheelKey = "front_right"; item.leftSide = false }
	}

	Loader {
		anchors { left: parent.left; leftMargin: 14; bottom: parent.bottom; bottomMargin: 18 }
		width: 124
		height: 72
		sourceComponent: wheelTile
		onLoaded: { item.wheelKey = "rear_left"; item.leftSide = true }
	}

	Loader {
		anchors { right: parent.right; rightMargin: 14; bottom: parent.bottom; bottomMargin: 18 }
		width: 124
		height: 72
		sourceComponent: wheelTile
		onLoaded: { item.wheelKey = "rear_right"; item.leftSide = false }
	}
}
