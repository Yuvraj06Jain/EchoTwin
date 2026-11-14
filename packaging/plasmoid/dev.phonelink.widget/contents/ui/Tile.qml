// The shared tile frame of the dashboard: rounded, a faint fill and hairline border that
// work on light and dark Plasma themes alike.
import QtQuick
import org.kde.kirigami as Kirigami

Rectangle {
    id: tile

    default property alias content: inner.data
    property real padding: 12
    readonly property color ink: Kirigami.Theme.textColor

    radius: 14
    color: Qt.rgba(ink.r, ink.g, ink.b, 0.065)
    border.width: 1
    border.color: Qt.rgba(ink.r, ink.g, ink.b, 0.09)

    Item {
        id: inner
        anchors.fill: parent
        anchors.margins: tile.padding
    }
}
