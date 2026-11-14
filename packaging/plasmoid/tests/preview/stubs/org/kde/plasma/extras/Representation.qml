import QtQuick
Item {
    id: rep
    property Item header
    property bool collapseMarginsHint
    default property alias contentData: body.data
    Item { id: headerHolder; anchors.top: parent.top; anchors.left: parent.left; anchors.right: parent.right; height: 42 }
    Item { id: body; anchors.top: headerHolder.bottom; anchors.bottom: parent.bottom; anchors.left: parent.left; anchors.right: parent.right; anchors.margins: 8 }
    onHeaderChanged: if (header) { header.parent = headerHolder; header.anchors.fill = headerHolder }
}
