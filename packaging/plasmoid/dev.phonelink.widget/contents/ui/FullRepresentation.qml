// The popup, "Tile dashboard" style (docs/design/widget-concepts.html, style 1):
// battery ring and now playing, notifications and messages, a drop tile and quick
// buttons. Captions open the full lists (a page with a back arrow). Files can be dropped
// anywhere on it.
import QtQuick
import QtQuick.Layouts
import QtQuick.Dialogs
import QtQuick.Controls as QQC2
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.extras as PlasmaExtras
import org.kde.kirigami as Kirigami
import org.kde.plasma.plasmoid

import "shell.js" as Shell

PlasmaExtras.Representation {
    id: full

    Layout.preferredWidth: Kirigami.Units.gridUnit * 29
    Layout.preferredHeight: Kirigami.Units.gridUnit * 28
    Layout.minimumWidth: Kirigami.Units.gridUnit * 26
    Layout.minimumHeight: Kirigami.Units.gridUnit * 24

    readonly property var media: root.st.media
    readonly property var notifications: root.st.notifications || []
    readonly property var sms: root.st.sms || []
    readonly property var transfers: root.st.transfers || []
    readonly property var activeTransfer: {
        for (var i = 0; i < transfers.length; i++)
            if (transfers[i].state === "active") return transfers[i];
        return null;
    }
    readonly property int unreadSms: {
        var n = 0;
        for (var i = 0; i < sms.length; i++) n += sms[i].unread;
        return n;
    }
    collapseMarginsHint: true

    // Open a full list over the dashboard: "notifications", "messages" or "transfers".
    function openList(which) {
        var page = { notifications: notificationsPage, messages: messagesPage, transfers: transfersPage }[which];
        if (page && stack.depth === 1) stack.push(page);
    }

    // Translucent popup: fade the theme's popup background (KDE still blurs behind it when
    // its Blur effect is on). The background is the window's themed frame item.
    readonly property real backgroundOpacity: Plasmoid.configuration.backgroundOpacity / 100
    function applyOpacity() {
        var w = full.Window.window;
        if (!w || !w.contentItem) return;
        var kids = w.contentItem.children;
        for (var i = 0; i < kids.length; i++) {
            if (kids[i].imagePath !== undefined) kids[i].opacity = backgroundOpacity;
        }
    }
    onBackgroundOpacityChanged: applyOpacity()
    onVisibleChanged: {
        if (visible) applyOpacity();
        else stack.pop(null); // opening the popup always starts on the dashboard
    }
    Window.onWindowChanged: applyOpacity()
    Component.onCompleted: applyOpacity()

    // "Connected · Wi-Fi" and friends.
    readonly property var pill: {
        if (!root.serviceUp) return { text: "Not running", color: root.grey };
        if (!root.st.paired) return { text: "Not paired", color: root.grey };
        if (root.switchedOff) return { text: "Connection off", color: root.grey };
        if (!root.online && root.phone.paused_by_peer) return { text: "Off on the phone", color: root.grey };
        if (!root.online) return { text: "Offline", color: root.grey };
        if (root.phone.via === "bluetooth") return { text: "Connected · Bluetooth", color: root.amber };
        return { text: "Connected · Wi-Fi", color: root.green };
    }

    header: PlasmaExtras.PlasmoidHeading {
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 6
            spacing: Kirigami.Units.smallSpacing

            Kirigami.Heading {
                level: 4
                text: root.phone ? root.phone.name : "EchoTwin"
                elide: Text.ElideRight
                Layout.fillWidth: true
                Layout.maximumWidth: implicitWidth
            }
            Rectangle {
                Layout.preferredHeight: 22
                Layout.preferredWidth: pillRow.implicitWidth + 18
                radius: 11
                color: Qt.rgba(full.pill.color.r, full.pill.color.g, full.pill.color.b, 0.16)
                Row {
                    id: pillRow
                    anchors.centerIn: parent
                    spacing: 6
                    Rectangle {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 6
                        height: 6
                        radius: 3
                        color: full.pill.color
                    }
                    PlasmaComponents.Label {
                        text: full.pill.text
                        color: full.pill.color
                        font.pixelSize: 12
                        font.bold: true
                    }
                }
            }
            Item { Layout.fillWidth: true }
            PlasmaComponents.ToolButton {
                icon.name: "view-refresh"
                visible: root.st.paired && root.serviceUp && !root.online && !root.switchedOff
                onClicked: root.run(["reconnect"])
                PlasmaComponents.ToolTip { text: "Reconnect now" }
            }
            PlasmaComponents.ToolButton {
                icon.name: "system-shutdown"
                visible: root.st.paired && root.serviceUp
                checkable: true
                checked: !root.switchedOff
                onClicked: root.run([root.switchedOff ? "on" : "off"])
                PlasmaComponents.ToolTip { text: root.switchedOff ? "Turn the connection on" : "Turn the connection off (stop searching)" }
            }
            PlasmaComponents.ToolButton {
                icon.name: "window-new"
                onClicked: root.openApp("")
                PlasmaComponents.ToolTip { text: "Open EchoTwin" }
            }
        }
    }

    // Not running / not paired: one clear message instead of empty tiles.
    Empty {
        anchors.centerIn: parent
        visible: !root.serviceUp || !root.st.paired
        iconName: "smartphone"
        text: !root.serviceUp ? "EchoTwin isn't running" : "No phone paired"
        explanation: !root.serviceUp ? "Start it with: systemctl --user start phonelinkd" : "Open EchoTwin to pair your phone"
    }

    QQC2.StackView {
        id: stack
        anchors.fill: parent
        anchors.margins: 4
        visible: root.serviceUp && root.st.paired
        initialItem: dashboard
    }

    // ---------------------------------------------------------------- the dashboard
    Component {
        id: dashboard

        ColumnLayout {
            spacing: 10

            CallStrip {
                Layout.fillWidth: true
                Layout.fillHeight: false
                Layout.preferredHeight: 60
                call: root.online ? (root.st.call || null) : null
            }

            // Rows with a fixed height must say so: a layout whose children fill would
            // otherwise grow too and squeeze the middle row.
            RowLayout {
                Layout.fillWidth: true
                Layout.fillHeight: false
                Layout.preferredHeight: 150
                spacing: 10

                Tile {
                    Layout.preferredWidth: 150
                    Layout.fillHeight: true
                    ColumnLayout {
                        anchors.fill: parent
                        spacing: 6
                        BatteryRing {
                            Layout.alignment: Qt.AlignHCenter
                            Layout.preferredWidth: 92
                            Layout.preferredHeight: 92
                            level: root.online && root.status && root.status.battery !== null && root.status.battery !== undefined ? root.status.battery : -1
                            charging: !!(root.status && root.status.charging)
                        }
                        RowLayout {
                            Layout.alignment: Qt.AlignHCenter
                            spacing: 6
                            visible: root.online && !!root.status
                            PlasmaComponents.Label {
                                text: root.status ? (root.status.network || "No network") : ""
                                opacity: 0.75
                                font: Kirigami.Theme.smallFont
                            }
                            Row {
                                spacing: 2
                                visible: !!root.status && root.status.network !== "Wi-Fi" && root.status.network !== ""
                                Repeater {
                                    model: 4
                                    Rectangle {
                                        anchors.bottom: parent.bottom
                                        width: 3
                                        height: 4 + index * 2.5
                                        radius: 1
                                        color: Kirigami.Theme.textColor
                                        opacity: root.status && index < root.status.signal ? 0.85 : 0.2
                                    }
                                }
                            }
                        }
                    }
                }

                NowPlayingTile {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    media: root.online ? full.media : null
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 10

                ListTile {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    title: "NOTIFICATIONS"
                    count: full.notifications.length
                    onMore: full.openList("notifications")

                    Empty {
                        anchors.centerIn: parent
                        visible: full.notifications.length === 0
                        iconName: "notifications"
                        text: root.online ? "All caught up" : "Your phone is offline"
                    }
                    Column {
                        width: parent.width
                        spacing: 10
                        Repeater {
                            model: full.notifications.slice(0, 3)
                            delegate: MouseArea {
                                width: parent.width
                                height: row.implicitHeight
                                cursorShape: Qt.PointingHandCursor
                                onClicked: full.openList("notifications")
                                RowLayout {
                                    id: row
                                    width: parent.width
                                    spacing: 10
                                    Chip {
                                        Layout.alignment: Qt.AlignTop
                                        name: modelData.app
                                        icon: modelData.icon || ""
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        spacing: 1
                                        RowLayout {
                                            Layout.fillWidth: true
                                            spacing: 4
                                            // Its natural width, shrinking (with "…") only when short of room.
                                            PlasmaComponents.Label {
                                                Layout.preferredWidth: implicitWidth
                                                Layout.minimumWidth: 30
                                                elide: Text.ElideRight
                                                font.bold: true
                                                text: modelData.title
                                            }
                                            PlasmaComponents.Label {
                                                text: "· " + Shell.ago(modelData.posted_ms)
                                                opacity: 0.5
                                                font: Kirigami.Theme.smallFont
                                            }
                                            Item { Layout.fillWidth: true }
                                        }
                                        PlasmaComponents.Label {
                                            Layout.fillWidth: true
                                            elide: Text.ElideRight
                                            maximumLineCount: 1
                                            opacity: 0.8
                                            text: modelData.text
                                        }
                                    }
                                    CodeChip {
                                        Layout.alignment: Qt.AlignVCenter
                                        visible: !!modelData.code
                                        code: modelData.code || ""
                                        notificationId: modelData.id
                                    }
                                }
                            }
                        }
                    }
                }

                ListTile {
                    Layout.preferredWidth: 150
                    Layout.fillHeight: true
                    title: "MESSAGES"
                    count: full.unreadSms
                    onMore: full.openList("messages")

                    Empty {
                        anchors.centerIn: parent
                        visible: full.sms.length === 0
                        iconName: "mail-message"
                        text: "No texts"
                    }
                    Column {
                        width: parent.width
                        spacing: 8
                        Repeater {
                            model: full.sms.slice(0, 3)
                            delegate: MouseArea {
                                width: parent.width
                                height: 28
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    if (modelData.unread > 0) root.run(["sms-read", modelData.id]);
                                    root.openApp("messages");
                                }
                                RowLayout {
                                    anchors.fill: parent
                                    spacing: 8
                                    Chip {
                                        round: true
                                        name: modelData.name
                                    }
                                    PlasmaComponents.Label {
                                        Layout.fillWidth: true
                                        elide: Text.ElideRight
                                        font.bold: modelData.unread > 0
                                        text: modelData.name
                                    }
                                }
                            }
                        }
                    }
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.fillHeight: false
                Layout.preferredHeight: 118
                spacing: 10

                DropTile {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    transfer: full.activeTransfer
                    onPick: picker.open()
                    onAll: full.openList("transfers")
                }
                QuickButtons {
                    Layout.preferredWidth: 150
                    Layout.fillHeight: true
                }
            }

            PlasmaComponents.Label {
                Layout.fillWidth: true
                visible: root.note !== ""
                horizontalAlignment: Text.AlignHCenter
                elide: Text.ElideRight
                font: Kirigami.Theme.smallFont
                color: root.green
                text: root.note
            }
        }
    }

    // A "Copy 482913" chip for one-time codes.
    component CodeChip: Rectangle {
        id: cc
        property string code
        property string notificationId
        implicitHeight: 24
        implicitWidth: ccLabel.implicitWidth + 16
        radius: 12
        color: ccArea.containsMouse ? Qt.rgba(0.24, 0.73, 0.51, 0.3) : Qt.rgba(0.24, 0.73, 0.51, 0.18)
        PlasmaComponents.Label {
            id: ccLabel
            anchors.centerIn: parent
            text: "Copy " + cc.code
            color: root.green
            font.pixelSize: 11
            font.bold: true
        }
        MouseArea {
            id: ccArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: {
                root.run(["copy-code", cc.notificationId]);
                root.flash("Code copied");
            }
        }
    }

    // ---------------------------------------------------------------- full lists
    Component {
        id: notificationsPage
        ListPage {
            title: "Notifications"
            onBack: stack.pop()
            PlasmaComponents.ScrollView {
                anchors.fill: parent
                ListView {
                    id: notifList
                    model: full.notifications
                    clip: true
                    Empty {
                        anchors.centerIn: parent
                        visible: notifList.count === 0
                        iconName: "notifications"
                        text: root.online ? "No notifications" : "Your phone is offline"
                    }
                    delegate: PlasmaComponents.ItemDelegate {
                        width: ListView.view.width
                        onClicked: root.openApp("notifications")
                        contentItem: RowLayout {
                            spacing: 10
                            Chip {
                                Layout.alignment: Qt.AlignTop
                                name: modelData.app
                                icon: modelData.icon || ""
                            }
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 0
                                PlasmaComponents.Label {
                                    Layout.fillWidth: true
                                    elide: Text.ElideRight
                                    font: Kirigami.Theme.smallFont
                                    opacity: 0.7
                                    text: modelData.app + "  ·  " + Shell.ago(modelData.posted_ms)
                                }
                                PlasmaComponents.Label {
                                    Layout.fillWidth: true
                                    elide: Text.ElideRight
                                    font.bold: true
                                    text: modelData.title
                                }
                                PlasmaComponents.Label {
                                    Layout.fillWidth: true
                                    wrapMode: Text.Wrap
                                    text: modelData.text
                                }
                            }
                            CodeChip {
                                visible: !!modelData.code
                                code: modelData.code || ""
                                notificationId: modelData.id
                            }
                            PlasmaComponents.ToolButton {
                                icon.name: "window-close"
                                onClicked: root.run(["dismiss", modelData.id])
                                PlasmaComponents.ToolTip { text: "Dismiss" }
                            }
                        }
                    }
                }
            }
        }
    }

    Component {
        id: messagesPage
        ListPage {
            title: "Messages"
            onBack: stack.pop()
            PlasmaComponents.ScrollView {
                anchors.fill: parent
                ListView {
                    id: smsList
                    model: full.sms
                    clip: true
                    Empty {
                        anchors.centerIn: parent
                        visible: smsList.count === 0
                        iconName: "mail-message"
                        text: "No conversations"
                        explanation: "Allow SMS access in the EchoTwin app on your phone"
                    }
                    delegate: PlasmaComponents.ItemDelegate {
                        width: ListView.view.width
                        onClicked: {
                            if (modelData.unread > 0) root.run(["sms-read", modelData.id]);
                            root.openApp("messages");
                        }
                        contentItem: RowLayout {
                            spacing: 10
                            Chip {
                                round: true
                                name: modelData.name
                            }
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 0
                                RowLayout {
                                    PlasmaComponents.Label {
                                        Layout.fillWidth: true
                                        elide: Text.ElideRight
                                        font.bold: modelData.unread > 0
                                        text: modelData.name
                                    }
                                    PlasmaComponents.Label {
                                        font: Kirigami.Theme.smallFont
                                        opacity: 0.7
                                        text: Shell.ago(modelData.date_ms)
                                    }
                                    Rectangle {
                                        visible: modelData.unread > 0
                                        width: 8
                                        height: 8
                                        radius: 4
                                        color: root.green
                                    }
                                }
                                PlasmaComponents.Label {
                                    Layout.fillWidth: true
                                    elide: Text.ElideRight
                                    opacity: 0.8
                                    text: (modelData.from_me ? "You: " : "") + modelData.text
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Component {
        id: transfersPage
        ListPage {
            title: "Transfers"
            onBack: stack.pop()
            ColumnLayout {
                anchors.fill: parent
                spacing: Kirigami.Units.smallSpacing
                PlasmaComponents.Button {
                    Layout.fillWidth: true
                    icon.name: "document-send"
                    text: "Send files to phone…"
                    onClicked: picker.open()
                }
                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    opacity: 0.7
                    font: Kirigami.Theme.smallFont
                    text: root.online
                        ? "Files from the phone are saved in Downloads/EchoTwin."
                        : "Your phone is offline: files are queued and sent when it reconnects."
                }
                PlasmaComponents.ScrollView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    ListView {
                        model: full.transfers
                        clip: true
                        spacing: Kirigami.Units.smallSpacing
                        delegate: ColumnLayout {
                            id: tr
                            width: ListView.view.width
                            spacing: 2
                            readonly property bool active: ["active", "waiting", "queued"].indexOf(modelData.state) >= 0
                            RowLayout {
                                PlasmaComponents.Label {
                                    Layout.fillWidth: true
                                    elide: Text.ElideMiddle
                                    text: (modelData.outgoing ? "↑ " : "↓ ") + modelData.name
                                }
                                PlasmaComponents.Label {
                                    font: Kirigami.Theme.smallFont
                                    opacity: 0.75
                                    text: {
                                        switch (modelData.state) {
                                        case "done": return modelData.outgoing ? "Sent" : "Saved";
                                        case "active": return Shell.percent(modelData.done, modelData.size) + "%" + Shell.left(modelData.eta_secs, modelData.rate);
                                        case "queued": return "Queued";
                                        case "waiting": return modelData.message || "Waiting";
                                        case "failed": return "Failed";
                                        default: return "Cancelled";
                                        }
                                    }
                                }
                                PlasmaComponents.ToolButton {
                                    visible: tr.active
                                    icon.name: "dialog-cancel"
                                    onClicked: root.run(["cancel-transfer", modelData.id])
                                    PlasmaComponents.ToolTip { text: "Cancel" }
                                }
                            }
                            PlasmaComponents.ProgressBar {
                                visible: tr.active
                                Layout.fillWidth: true
                                from: 0
                                to: 100
                                value: Shell.percent(modelData.done, modelData.size)
                            }
                        }
                    }
                }
            }
        }
    }

    // Dropping files anywhere on the popup sends them too.
    DropArea {
        anchors.fill: parent
        z: -1
        keys: ["text/uri-list"]
        onDropped: (d) => {
            if (d.hasUrls) root.sendUrls(d.urls);
            d.acceptProposedAction();
        }
    }

    FileDialog {
        id: picker
        title: "Send files to your phone"
        fileMode: FileDialog.OpenFiles
        onAccepted: root.sendUrls(selectedFiles)
    }
}
