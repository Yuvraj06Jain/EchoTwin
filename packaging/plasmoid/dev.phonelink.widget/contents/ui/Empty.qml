// A quiet empty state: small green icon and muted text.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

ColumnLayout {
    property alias iconName: icon.source
    property alias text: label.text
    property string explanation: ""
    spacing: Kirigami.Units.smallSpacing

    Kirigami.Icon {
        id: icon
        Layout.alignment: Qt.AlignHCenter
        Layout.preferredWidth: 22
        Layout.preferredHeight: 22
        isMask: true
        color: root.green
    }
    PlasmaComponents.Label {
        id: label
        Layout.alignment: Qt.AlignHCenter
        opacity: 0.6
    }
    PlasmaComponents.Label {
        visible: text !== ""
        text: parent.explanation
        Layout.alignment: Qt.AlignHCenter
        Layout.maximumWidth: Kirigami.Units.gridUnit * 16
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
        font: Kirigami.Theme.smallFont
        opacity: 0.5
    }
}
