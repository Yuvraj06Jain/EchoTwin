pragma Singleton
import QtQuick
QtObject {
    property color textColor: "#eef1f3"
    property color backgroundColor: "#1d2125"
    property font defaultFont: Qt.font({ family: "Noto Sans", pixelSize: 13 })
    property font smallFont: Qt.font({ family: "Noto Sans", pixelSize: 11 })
}
