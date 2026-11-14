import QtQuick
import QtQuick.Controls as QQC2
import org.kde.kirigami as K
QQC2.ToolButton { id: b; implicitWidth: 30; implicitHeight: 30; contentItem: K.Icon { source: b.icon.name; width: 16; height: 16 } background: Rectangle { radius: 4; color: b.checked ? Qt.rgba(1,1,1,0.12) : "transparent" } }
