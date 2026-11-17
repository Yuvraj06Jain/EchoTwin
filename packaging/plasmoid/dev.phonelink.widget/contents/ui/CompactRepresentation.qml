// The panel icon, "live": a small phone outline that fills from the bottom with the phone's
// battery level (amber when low), a count badge for new things, one pulse when something
// arrives, and a ring showing a file transfer's progress. Grey and empty while offline.
// Everything follows the widget's change-wait loop: no timers of its own.
import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import org.kde.plasma.plasmoid
import org.kde.kirigami as Kirigami

import "shell.js" as Shell

MouseArea {
    id: compact

    readonly property bool dropping: drop.containsDrag
    readonly property bool low: root.battery >= 0 && root.battery < 20 && !(root.status && root.status.charging)
    readonly property color lineColor: dropping ? root.green : (!root.online ? root.grey : (low ? root.amber : root.green))
    readonly property int count: root.online && !root.expanded ? root.newCount : 0

    hoverEnabled: true
    acceptedButtons: Qt.LeftButton | Qt.MiddleButton
    onClicked: (mouse) => {
        if (mouse.button === Qt.MiddleButton) root.openApp("");
        else root.expanded = !root.expanded;
    }

    Layout.minimumWidth: Kirigami.Units.iconSizes.small
    Layout.preferredWidth: height * 0.7

    // One pulse when the count goes up.
    property int lastCount: 0
    onCountChanged: {
        if (count > lastCount) pulse.restart();
        lastCount = count;
    }

    Rectangle {
        id: body
        anchors.centerIn: parent
        // A bit smaller than a tray icon (~20 px in a 44 px panel).
        height: Math.max(12, Math.round(Math.min(parent.height * 0.46, parent.width / 0.7 * 0.9)))
        width: Math.round(height * 0.7)
        radius: width * 0.26
        color: compact.dropping ? Qt.rgba(0.24, 0.73, 0.51, 0.18) : "transparent"
        border.width: 1.5
        border.color: compact.lineColor
        opacity: root.switchedOff ? 0.45 : (compact.containsMouse || root.expanded ? 1 : 0.92)

        // Battery fill, inset so it stays inside the rounded outline.
        Rectangle {
            visible: root.online && root.battery >= 0 && !compact.dropping
            anchors.bottom: parent.bottom
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottomMargin: 2.5
            width: parent.width - 5
            height: Math.max(1.5, (parent.height - 5) * root.battery / 100)
            radius: Math.max(1, body.radius - 2.5)
            color: compact.lineColor
            opacity: 0.8
        }
    }

    // The pulse: a ring that grows and fades once.
    Rectangle {
        id: ripple
        anchors.centerIn: body
        width: body.width
        height: body.height
        radius: body.radius
        color: "transparent"
        border.width: 1.5
        border.color: root.green
        opacity: 0
        SequentialAnimation {
            id: pulse
            ParallelAnimation {
                NumberAnimation { target: ripple; property: "scale"; from: 1; to: 1.5; duration: 600; easing.type: Easing.OutCubic }
                NumberAnimation { target: ripple; property: "opacity"; from: 0.8; to: 0; duration: 600 }
            }
        }
    }

    // A file moving: a thin progress ring around the icon.
    Shape {
        visible: !!root.activeTransfer
        anchors.centerIn: body
        width: Math.max(body.width, body.height) + 8
        height: width
        antialiasing: true
        ShapePath {
            strokeColor: root.green
            strokeWidth: 1.5
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            PathAngleArc {
                centerX: (body.height + 8) / 2
                centerY: (body.height + 8) / 2
                radiusX: (body.height + 8) / 2 - 1
                radiusY: (body.height + 8) / 2 - 1
                startAngle: -90
                sweepAngle: root.activeTransfer ? 3.6 * Shell.percent(root.activeTransfer.done, root.activeTransfer.size) : 0
            }
        }
    }

    // On a call: a small green handset at the bottom corner.
    Rectangle {
        visible: root.online && !!root.st.call && root.st.call.phase === "active"
        height: Math.max(10, Math.round(body.height * 0.55))
        width: height
        radius: height / 2
        color: root.green
        border.width: 1.5
        border.color: Kirigami.Theme.backgroundColor
        x: body.x + body.width - width * 0.45
        y: body.y + body.height - height * 0.6
        Text {
            anchors.centerIn: parent
            text: "\u2706"
            color: "white"
            font.pixelSize: parent.height * 0.62
        }
    }

    // How many new things since you last opened the popup.
    Rectangle {
        visible: compact.count > 0
        height: Math.max(10, Math.round(body.height * 0.55))
        width: Math.max(height, badgeText.implicitWidth + 5)
        radius: height / 2
        color: root.green
        border.width: 1.5
        border.color: Kirigami.Theme.backgroundColor
        x: body.x + body.width - width * 0.45
        y: body.y - height * 0.4
        Text {
            id: badgeText
            anchors.centerIn: parent
            text: Shell.badge(compact.count)
            color: "white"
            font.pixelSize: parent.height * 0.68
            font.bold: true
        }
    }

    DropArea {
        id: drop
        anchors.fill: parent
        keys: ["text/uri-list"]
        onDropped: (d) => {
            if (d.hasUrls) root.sendUrls(d.urls);
            d.acceptProposedAction();
        }
    }
}
