// A dashboard tile with a caption ("NOTIFICATIONS 3 ›"); clicking the caption opens the
// full list.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

Tile {
    id: tile

    property string title
    property int count: 0
    default property alias body: holder.data
    signal more()

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        MouseArea {
            Layout.fillWidth: true
            Layout.preferredHeight: caption.implicitHeight
            cursorShape: Qt.PointingHandCursor
            hoverEnabled: true
            onClicked: tile.more()
            RowLayout {
                id: caption
                anchors.fill: parent
                spacing: 6
                PlasmaComponents.Label {
                    text: tile.title
                    font.pixelSize: 11
                    font.bold: true
                    font.letterSpacing: 1
                    opacity: 0.6
                }
                Rectangle {
                    visible: tile.count > 0
                    Layout.preferredHeight: 17
                    Layout.preferredWidth: Math.max(17, countLabel.implicitWidth + 9)
                    radius: 8.5
                    color: root.green
                    PlasmaComponents.Label {
                        id: countLabel
                        anchors.centerIn: parent
                        text: tile.count > 99 ? "99+" : String(tile.count)
                        color: "white"
                        font.pixelSize: 10
                        font.bold: true
                    }
                }
                Item { Layout.fillWidth: true }
                Kirigami.Icon {
                    Layout.preferredWidth: 14
                    Layout.preferredHeight: 14
                    source: "go-next"
                    opacity: parent.parent.containsMouse ? 0.9 : 0.45
                }
            }
        }
        Item {
            id: holder
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
        }
    }
}
