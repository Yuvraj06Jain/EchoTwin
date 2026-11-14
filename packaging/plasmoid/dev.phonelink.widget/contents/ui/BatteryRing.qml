// A battery ring: the level as an arc, the number in the middle, "charging" under it.
// Amber below 20% while not charging; "–" when the phone doesn't report a battery.
import QtQuick
import QtQuick.Shapes
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

Item {
    id: ring

    property int level: -1
    property bool charging: false
    property real stroke: 7
    readonly property bool low: level >= 0 && level < 20 && !charging
    readonly property color ink: Kirigami.Theme.textColor

    implicitWidth: 92
    implicitHeight: 92

    Shape {
        anchors.fill: parent
        antialiasing: true
        layer.enabled: true
        layer.samples: 4

        ShapePath {
            strokeColor: Qt.rgba(ring.ink.r, ring.ink.g, ring.ink.b, 0.1)
            strokeWidth: ring.stroke
            fillColor: "transparent"
            PathAngleArc {
                centerX: ring.width / 2
                centerY: ring.height / 2
                radiusX: ring.width / 2 - ring.stroke / 2
                radiusY: ring.height / 2 - ring.stroke / 2
                startAngle: 0
                sweepAngle: 360
            }
        }
        ShapePath {
            strokeColor: ring.level < 0 ? "transparent" : (ring.low ? root.amber : root.green)
            strokeWidth: ring.stroke
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            PathAngleArc {
                centerX: ring.width / 2
                centerY: ring.height / 2
                radiusX: ring.width / 2 - ring.stroke / 2
                radiusY: ring.height / 2 - ring.stroke / 2
                startAngle: -90
                sweepAngle: Math.max(0, Math.min(100, ring.level)) * 3.6
            }
        }
    }

    Column {
        anchors.centerIn: parent
        spacing: 0
        PlasmaComponents.Label {
            anchors.horizontalCenter: parent.horizontalCenter
            text: ring.level < 0 ? "–" : ring.level + "%"
            font.pixelSize: ring.height * 0.26
            font.bold: true
        }
        PlasmaComponents.Label {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: ring.charging && ring.level >= 0
            text: "charging"
            font.pixelSize: Math.max(9, ring.height * 0.12)
            opacity: 0.7
        }
    }
}
