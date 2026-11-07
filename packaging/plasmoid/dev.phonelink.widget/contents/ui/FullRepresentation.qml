// The popup: phone status, music (only while something plays), then Notifications,
// Messages and Files. Files can also be dropped anywhere on it.
import QtQuick
import QtQuick.Layouts
import QtQuick.Dialogs
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.extras as PlasmaExtras
import org.kde.kirigami as Kirigami
import org.kde.plasma.plasmoid

import "shell.js" as Shell

PlasmaExtras.Representation {
    id: full

    Layout.preferredWidth: Kirigami.Units.gridUnit * 26
    Layout.preferredHeight: Kirigami.Units.gridUnit * 20
    Layout.minimumWidth: Kirigami.Units.gridUnit * 22
    Layout.minimumHeight: Kirigami.Units.gridUnit * 16

    readonly property var media: root.st.media
    collapseMarginsHint: true

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
    onVisibleChanged: if (visible) applyOpacity()
    Window.onWindowChanged: applyOpacity()
    Component.onCompleted: applyOpacity()

    header: PlasmaExtras.PlasmoidHeading {
        RowLayout {
            anchors.fill: parent
            spacing: Kirigami.Units.largeSpacing

            Rectangle {
                Layout.leftMargin: Kirigami.Units.smallSpacing
                Layout.alignment: Qt.AlignVCenter
                Layout.preferredWidth: 13
                Layout.preferredHeight: 20
                radius: width * 0.22
                color: "transparent"
                border.width: 1.5
                border.color: root.online ? root.green : root.grey
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0
                Kirigami.Heading {
                    level: 4
                    text: root.phone ? root.phone.name : "PhoneLink"
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    opacity: 0.75
                    font: Kirigami.Theme.smallFont
                    text: {
                        if (!root.serviceUp) return "PhoneLink service isn't running";
                        if (!root.st.paired) return "Pair your phone in the PhoneLink app";
                        if (!root.online) return "Offline · files and notes wait for it";
                        var p = [];
                        var s = root.status;
                        if (s && s.battery !== null && s.battery !== undefined)
                            p.push(s.battery + "%" + (s.charging ? " charging" : ""));
                        if (s && s.network && s.network !== "Wi-Fi")
                            p.push(s.network + (s.signal > 0 ? " " + "▮".repeat(s.signal) : ""));
                        p.push(root.phone.via === "bluetooth" ? "Bluetooth" : "Wi-Fi");
                        return p.join("  ·  ");
                    }
                }
            }
            PlasmaComponents.ToolButton {
                icon.name: "view-refresh"
                visible: root.st.paired && !root.online
                onClicked: root.run(["reconnect"])
                PlasmaComponents.ToolTip { text: "Reconnect now" }
            }
            PlasmaComponents.ToolButton {
                icon.name: "window-new"
                onClicked: root.openApp("")
                PlasmaComponents.ToolTip { text: "Open PhoneLink" }
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: Kirigami.Units.smallSpacing

        // ------------------------------------------------------------ music
        Rectangle {
            visible: !!full.media
            Layout.fillWidth: true
            Layout.preferredHeight: musicRow.implicitHeight + Kirigami.Units.largeSpacing * 2
            radius: Kirigami.Units.cornerRadius
            color: Qt.rgba(Kirigami.Theme.textColor.r, Kirigami.Theme.textColor.g, Kirigami.Theme.textColor.b, 0.06)

            RowLayout {
                id: musicRow
                anchors.fill: parent
                anchors.margins: Kirigami.Units.largeSpacing
                spacing: Kirigami.Units.largeSpacing

                Image {
                    Layout.preferredWidth: Kirigami.Units.iconSizes.large
                    Layout.preferredHeight: Kirigami.Units.iconSizes.large
                    source: full.media && full.media.art ? full.media.art : ""
                    visible: source != ""
                    fillMode: Image.PreserveAspectCrop
                    sourceSize.width: 128
                    sourceSize.height: 128
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 2
                    PlasmaComponents.Label {
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        font.bold: true
                        text: full.media ? (full.media.title || full.media.app) : ""
                    }
                    PlasmaComponents.Label {
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        opacity: 0.75
                        text: full.media ? [full.media.artist, full.media.app].filter((x) => !!x).join(" · ") : ""
                    }
                    RowLayout {
                        spacing: 0
                        PlasmaComponents.ToolButton {
                            icon.name: "media-skip-backward"
                            enabled: !!full.media && full.media.can_previous
                            onClicked: root.run(["media", "previous"])
                        }
                        PlasmaComponents.ToolButton {
                            icon.name: full.media && full.media.playing ? "media-playback-pause" : "media-playback-start"
                            onClicked: root.run(["media", "play-pause"])
                        }
                        PlasmaComponents.ToolButton {
                            icon.name: "media-skip-forward"
                            enabled: !!full.media && full.media.can_next
                            onClicked: root.run(["media", "next"])
                        }
                        Kirigami.Icon {
                            source: "audio-volume-medium"
                            Layout.preferredWidth: Kirigami.Units.iconSizes.small
                            Layout.preferredHeight: Kirigami.Units.iconSizes.small
                            Layout.leftMargin: Kirigami.Units.smallSpacing
                        }
                        PlasmaComponents.Slider {
                            id: volume
                            Layout.fillWidth: true
                            from: 0
                            to: full.media ? Math.max(1, full.media.max_volume) : 15
                            stepSize: 1
                            // Follow the phone unless you're dragging it.
                            Binding on value {
                                when: !volume.pressed
                                value: full.media ? full.media.volume : 0
                            }
                            onPressedChanged: if (!pressed) root.run(["media", "volume", String(Math.round(value))])
                        }
                    }
                }
            }
        }

        // ------------------------------------------------------------ tabs
        PlasmaComponents.TabBar {
            id: tabs
            Layout.fillWidth: true
            PlasmaComponents.TabButton {
                text: "Notifications" + ((root.st.notifications || []).length ? " (" + root.st.notifications.length + ")" : "")
            }
            PlasmaComponents.TabButton {
                text: {
                    var n = 0;
                    (root.st.sms || []).forEach((t) => n += t.unread);
                    return "Messages" + (n ? " (" + n + ")" : "");
                }
            }
            PlasmaComponents.TabButton { text: "Files" }
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabs.currentIndex

            // Notifications
            PlasmaComponents.ScrollView {
                ListView {
                    id: notifList
                    model: root.st.notifications || []
                    spacing: Kirigami.Units.smallSpacing
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
                            spacing: Kirigami.Units.smallSpacing
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
                                    maximumLineCount: 3
                                    elide: Text.ElideRight
                                    text: modelData.text
                                }
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

            // Messages (SMS)
            PlasmaComponents.ScrollView {
                ListView {
                    id: smsList
                    model: root.st.sms || []
                    clip: true
                    Empty {
                        anchors.centerIn: parent
                        visible: smsList.count === 0
                        iconName: "mail-message"
                        text: "No conversations"
                        explanation: "Allow SMS access in the PhoneLink app on your phone"
                    }
                    delegate: PlasmaComponents.ItemDelegate {
                        width: ListView.view.width
                        onClicked: {
                            if (modelData.unread > 0) root.run(["sms-read", modelData.id]);
                            root.openApp("messages");
                        }
                        contentItem: ColumnLayout {
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
                                    width: 8; height: 8; radius: 4
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

            // Files
            ColumnLayout {
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
                        ? "Or drop files on the phone icon in the panel. Files from the phone are saved in Downloads/PhoneLink."
                        : "Your phone is offline: files are queued and sent when it reconnects."
                }
                PlasmaComponents.ScrollView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    ListView {
                        model: root.st.transfers || []
                        clip: true
                        spacing: Kirigami.Units.smallSpacing
                        delegate: ColumnLayout {
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
                                        case "active": return Shell.percent(modelData.done, modelData.size) + "%";
                                        case "queued": return "Queued";
                                        case "waiting": return modelData.message || "Waiting";
                                        case "failed": return "Failed";
                                        default: return "Cancelled";
                                        }
                                    }
                                }
                                PlasmaComponents.ToolButton {
                                    visible: parent.parent.active
                                    icon.name: "dialog-cancel"
                                    onClicked: root.run(["cancel-transfer", modelData.id])
                                    PlasmaComponents.ToolTip { text: "Cancel" }
                                }
                            }
                            PlasmaComponents.ProgressBar {
                                visible: parent.active
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

        PlasmaComponents.Label {
            Layout.fillWidth: true
            horizontalAlignment: root.note !== "" ? Text.AlignHCenter : Text.AlignLeft
            wrapMode: Text.Wrap
            font: Kirigami.Theme.smallFont
            color: root.note !== "" ? root.green : Kirigami.Theme.textColor
            opacity: root.note !== "" ? 1 : 0.55
            text: root.note !== "" ? root.note : "Drop files on the phone icon to send them"
        }
    }

    // Dropping files anywhere on the popup sends them too.
    DropArea {
        anchors.fill: parent
        keys: ["text/uri-list"]
        onDropped: (d) => {
            if (d.hasUrls) {
                root.sendUrls(d.urls);
                tabs.currentIndex = 2;
            }
            d.acceptProposedAction();
        }
    }

    FileDialog {
        id: picker
        title: "Send files to your phone"
        fileMode: FileDialog.OpenFiles
        onAccepted: {
            root.sendUrls(selectedFiles);
            tabs.currentIndex = 2;
        }
    }
}
