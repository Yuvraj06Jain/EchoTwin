// The call on the phone, at the top of the dashboard: who, "Incoming call" or the time on
// the call, and round buttons: Answer (green), Decline / End (red), as the phone's call screen
// offers them. Each runs `phonelink call <action>`.
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "shell.js" as Shell

Tile {
    id: strip

    property var call: null
    readonly property bool active: !!call && call.phase === "active"
    property double now: Date.now()

    function two(n) { return (n < 10 ? "0" : "") + n; }
    function elapsed() {
        if (!call || !call.active_ms) return "";
        var s = Math.max(0, Math.floor((now - call.active_ms) / 1000));
        var h = Math.floor(s / 3600);
        return (h ? h + ":" + two(Math.floor(s / 60) % 60) : two(Math.floor(s / 60))) + ":" + two(s % 60);
    }

    visible: !!call
    implicitHeight: 60
    padding: 10
    border.color: Qt.rgba(0.24, 0.73, 0.51, 0.45)

    // Ticks only while a call is active and the popup is open.
    Timer {
        interval: 1000
        repeat: true
        running: strip.active && root.expanded
        triggeredOnStart: true
        onTriggered: strip.now = Date.now()
    }

    RowLayout {
        anchors.fill: parent
        spacing: 10

        Rectangle {
            Layout.preferredWidth: 36
            Layout.preferredHeight: 36
            radius: 18
            color: Shell.colorFor(strip.call ? (strip.call.caller || "?") : "?")
            border.width: 3
            border.color: Qt.rgba(0.24, 0.73, 0.51, 0.35)
            PlasmaComponents.Label {
                anchors.centerIn: parent
                text: Shell.initial(strip.call ? (strip.call.caller || "?") : "?")
                color: "white"
                font.bold: true
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 0
            PlasmaComponents.Label {
                Layout.fillWidth: true
                text: strip.call ? (strip.call.caller || "Unknown caller") : ""
                font.bold: true
                elide: Text.ElideRight
            }
            PlasmaComponents.Label {
                Layout.fillWidth: true
                text: !strip.call ? "" : strip.call.phase === "ringing" ? "Incoming call" : ("On call" + (strip.call.active_ms ? " · " + strip.elapsed() : ""))
                color: strip.active ? Qt.rgba(0.24, 0.73, 0.51, 1) : Kirigami.Theme.textColor
                opacity: strip.active ? 1 : 0.7
                font: Kirigami.Theme.smallFont
                elide: Text.ElideRight
            }
        }

        Repeater {
            // Fixed order; only the buttons the phone offers right now.
            model: [["answer", "Answer", "\u2706", "ok"], ["decline", "Decline", "\u2715", "no"], ["end", "End call", "\u2715", "no"]]
                .filter(function (b) { return strip.call && (strip.call.actions || []).indexOf(b[0]) >= 0; })
            delegate: Rectangle {
                required property var modelData
                Layout.preferredWidth: 32
                Layout.preferredHeight: 32
                radius: 16
                color: modelData[3] === "ok" ? "#23905f" : modelData[3] === "no" ? "#d9534f" : Qt.rgba(strip.ink.r, strip.ink.g, strip.ink.b, 0.12)
                Accessible.role: Accessible.Button
                Accessible.name: modelData[1]
                PlasmaComponents.Label {
                    anchors.centerIn: parent
                    text: modelData[2]
                    color: modelData[3] === "nt" ? Kirigami.Theme.textColor : "white"
                }
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.run(["call", modelData[0]])
                }
                PlasmaComponents.ToolTip.text: modelData[1]
                PlasmaComponents.ToolTip.visible: hover.hovered
                HoverHandler { id: hover }
            }
        }
    }
}
