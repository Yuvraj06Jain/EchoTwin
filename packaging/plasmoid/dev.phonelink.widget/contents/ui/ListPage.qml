// A full list opened from the dashboard, with a back arrow.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

Item {
    id: page

    property string title
    default property alias body: holder.data
    signal back()

    ColumnLayout {
        anchors.fill: parent
        spacing: 6
        RowLayout {
            Layout.fillWidth: true
            PlasmaComponents.ToolButton {
                icon.name: "go-previous"
                onClicked: page.back()
                PlasmaComponents.ToolTip { text: "Back" }
            }
            Kirigami.Heading {
                level: 4
                text: page.title
                Layout.fillWidth: true
            }
        }
        Item {
            id: holder
            Layout.fillWidth: true
            Layout.fillHeight: true
        }
    }
}
