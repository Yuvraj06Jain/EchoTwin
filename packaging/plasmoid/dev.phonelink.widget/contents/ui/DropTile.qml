// "Drop files to send": a dashed accent frame. While a file moves it shows its name, the
// progress and the time left. Click to pick files.
import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

import "shell.js" as Shell

Item {
    id: tile

    property var transfer: null
    property bool hovering: drop.containsDrag || area.containsMouse
    signal pick()
    signal all()

    Rectangle {
        anchors.fill: parent
        radius: 14
        color: Qt.rgba(0.24, 0.73, 0.51, tile.hovering ? 0.16 : 0.08)
    }
    // Dashed rounded border (QML rectangles can't dash).
    Shape {
        anchors.fill: parent
        antialiasing: true
        ShapePath {
            strokeColor: Qt.rgba(0.24, 0.73, 0.51, 0.7)
            strokeWidth: 1.2
            strokeStyle: ShapePath.DashLine
            dashPattern: [4, 3]
            fillColor: "transparent"
            PathSvg {
                readonly property real w: tile.width - 1
                readonly property real h: tile.height - 1
                readonly property real r: 14
                path: "M " + (0.5 + r) + " 0.5 H " + (w - r) + " A " + r + " " + r + " 0 0 1 " + w + " " + (0.5 + r)
                    + " V " + (h - r) + " A " + r + " " + r + " 0 0 1 " + (w - r) + " " + h
                    + " H " + (0.5 + r) + " A " + r + " " + r + " 0 0 1 0.5 " + (h - r)
                    + " V " + (0.5 + r) + " A " + r + " " + r + " 0 0 1 " + (0.5 + r) + " 0.5 Z"
            }
        }
    }

    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: tile.pick()
    }

    RowLayout {
        anchors.fill: parent
        anchors.margins: 14
        spacing: 12
        Kirigami.Icon {
            Layout.preferredWidth: 22
            Layout.preferredHeight: 22
            source: "document-send"
            isMask: true
            color: root.green
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 3
            PlasmaComponents.Label {
                Layout.fillWidth: true
                elide: Text.ElideRight
                font.bold: true
                color: root.green
                text: drop.containsDrag ? "Release to send" : "Drop files to send"
            }
            PlasmaComponents.Label {
                Layout.fillWidth: true
                elide: Text.ElideMiddle
                font: Kirigami.Theme.smallFont
                color: root.green
                opacity: 0.85
                text: tile.transfer
                    ? tile.transfer.name + " · " + Shell.percent(tile.transfer.done, tile.transfer.size) + "%" + Shell.left(tile.transfer.eta_secs, tile.transfer.rate)
                    : (root.online ? "or click to choose" : "queued until your phone is back")
            }
            Rectangle {
                visible: !!tile.transfer
                Layout.fillWidth: true
                Layout.preferredHeight: 3
                radius: 1.5
                color: Qt.rgba(0.24, 0.73, 0.51, 0.25)
                Rectangle {
                    width: tile.transfer ? parent.width * Shell.percent(tile.transfer.done, tile.transfer.size) / 100 : 0
                    height: parent.height
                    radius: parent.radius
                    color: root.green
                }
            }
        }
        PlasmaComponents.ToolButton {
            icon.name: "view-list-details"
            onClicked: tile.all()
            PlasmaComponents.ToolTip { text: "All transfers" }
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
