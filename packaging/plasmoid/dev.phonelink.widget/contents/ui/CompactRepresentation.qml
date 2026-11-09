// The panel icon: a small phone outline (green while connected, grey offline), empty inside
// except for a dot when something new arrived. Drop files on it to send them.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.plasmoid
import org.kde.kirigami as Kirigami

MouseArea {
    id: compact

    readonly property bool dropping: drop.containsDrag
    readonly property color lineColor: root.online || dropping ? root.green : root.grey

    hoverEnabled: true
    acceptedButtons: Qt.LeftButton | Qt.MiddleButton
    onClicked: (mouse) => {
        if (mouse.button === Qt.MiddleButton) root.openApp("");
        else root.expanded = !root.expanded;
    }

    Layout.minimumWidth: Kirigami.Units.iconSizes.small
    Layout.preferredWidth: height * 0.6

    Rectangle {
        id: body
        anchors.centerIn: parent
        // A bit smaller than a tray icon (~20 px in a 44 px panel), shape "A" from the preview.
        height: Math.max(12, Math.round(Math.min(parent.height * 0.46, parent.width / 0.7 * 0.9)))
        width: Math.round(height * 0.7)
        radius: width * 0.26
        color: compact.dropping ? Qt.rgba(0.24, 0.73, 0.51, 0.18) : "transparent"
        border.width: 1.5
        border.color: compact.lineColor
        opacity: root.switchedOff ? 0.45 : (compact.containsMouse || root.expanded ? 1 : 0.92)

        // Camera.
        Rectangle {
            width: Math.max(2, body.width * 0.11)
            height: width
            radius: width / 2
            color: compact.lineColor
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: parent.top
            anchors.topMargin: body.border.width + Math.max(1, body.height * 0.05)
        }
    }

    // Something new (notification or unread SMS) since you last opened the popup.
    Rectangle {
        visible: root.unread && root.online && !root.expanded
        width: Math.max(5, Math.round(body.width * 0.42))
        height: width
        radius: width / 2
        color: root.green
        border.width: 1.5
        border.color: Kirigami.Theme.backgroundColor
        x: body.x + body.width - width * 0.6
        y: body.y - height * 0.3
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
