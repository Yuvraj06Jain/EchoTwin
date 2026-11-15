// Four quick buttons: Ring phone, Note, Battery saver, Open EchoTwin.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

Tile {
    id: tile

    property bool ringing: false
    padding: 6

    // The phone stops ringing by itself after a minute.
    Timer {
        id: ringStop
        interval: 60000
        onTriggered: tile.ringing = false
    }

    component Quick: Rectangle {
        id: q
        property string label
        property string iconName
        property bool on: false
        property bool active: true
        signal tapped()
        Layout.fillWidth: true
        Layout.fillHeight: true
        radius: 10
        opacity: active ? 1 : 0.4
        color: on ? Qt.rgba(0.24, 0.73, 0.51, 0.22)
                  : Qt.rgba(tile.ink.r, tile.ink.g, tile.ink.b, qa.containsMouse && active ? 0.1 : 0.05)
        ColumnLayout {
            anchors.centerIn: parent
            spacing: 3
            Kirigami.Icon {
                Layout.alignment: Qt.AlignHCenter
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
                source: q.iconName
                isMask: true
                color: q.on ? root.green : Kirigami.Theme.textColor
            }
            PlasmaComponents.Label {
                Layout.alignment: Qt.AlignHCenter
                text: q.label
                font.pixelSize: 11
                opacity: 0.85
            }
        }
        MouseArea {
            id: qa
            anchors.fill: parent
            hoverEnabled: true
            enabled: q.active
            cursorShape: Qt.PointingHandCursor
            onClicked: q.tapped()
        }
    }

    GridLayout {
        anchors.fill: parent
        columns: 2
        rowSpacing: 6
        columnSpacing: 6

        Quick {
            label: tile.ringing ? "Stop" : "Ring"
            iconName: tile.ringing ? "media-playback-stop" : "notifications"
            on: tile.ringing
            active: root.online && !!root.phone && !!root.phone.can_ring
            onTapped: {
                tile.ringing = !tile.ringing;
                root.run(["ring", tile.ringing ? "start" : "stop"]);
                root.flash(tile.ringing ? "Ringing your phone" : "Stopped ringing");
                if (tile.ringing) ringStop.restart();
            }
        }
        Quick {
            label: "Note"
            iconName: "document-edit"
            active: root.st.paired
            onTapped: root.openApp("note")
        }
        Quick {
            // On by itself while the laptop's battery is low: shown, but not switchable here.
            label: root.st.saver_auto ? "Saver · auto" : "Saver"
            iconName: "battery-profile-powersave"
            on: root.st.battery_saver !== false
            active: !root.st.saver_auto
            onTapped: {
                var on = root.st.battery_saver === false;
                root.run(["saver", on ? "on" : "off"]);
                root.flash(on ? "Battery saver on" : "Battery saver off");
            }
        }
        Quick {
            label: "Open"
            iconName: "window-new"
            onTapped: root.openApp("")
        }
    }
}
