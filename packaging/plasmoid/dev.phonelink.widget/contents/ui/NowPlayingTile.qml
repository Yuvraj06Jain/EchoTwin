// What plays on the phone: blurred album art behind, cover, title, artist · app, the
// controls and the phone's volume. A quiet "Nothing playing" otherwise.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

Tile {
    id: tile

    property var media: null
    readonly property bool hasArt: !!media && !!media.art

    padding: 0

    // Background: blurred art if the system can blur, else an accent tint.
    Rectangle {
        anchors.fill: parent
        radius: tile.radius
        visible: !!tile.media && (!tile.hasArt || backdrop.status !== Loader.Ready)
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0; color: Qt.rgba(0.24, 0.73, 0.51, 0.22) }
            GradientStop { position: 1; color: Qt.rgba(0.24, 0.73, 0.51, 0.05) }
        }
    }
    Loader {
        id: backdrop
        anchors.fill: parent
        active: tile.hasArt
        source: "ArtBackdrop.qml"
        onLoaded: {
            item.radius = Qt.binding(() => tile.radius);
            item.source = Qt.binding(() => tile.media && tile.media.art ? tile.media.art : "");
        }
    }
    Rectangle {
        anchors.fill: parent
        radius: tile.radius
        visible: backdrop.status === Loader.Ready
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0; color: Qt.rgba(0, 0, 0, 0.55) }
            GradientStop { position: 1; color: Qt.rgba(0, 0, 0, 0.2) }
        }
    }

    // Nothing playing.
    ColumnLayout {
        anchors.centerIn: parent
        visible: !tile.media
        spacing: 4
        Kirigami.Icon {
            Layout.alignment: Qt.AlignHCenter
            Layout.preferredWidth: 26
            Layout.preferredHeight: 26
            source: "media-playback-start"
            isMask: true
            color: root.green
        }
        PlasmaComponents.Label {
            Layout.alignment: Qt.AlignHCenter
            text: "Nothing playing"
            opacity: 0.6
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 14
        visible: !!tile.media
        spacing: 8

        RowLayout {
            Layout.fillWidth: true
            spacing: 12
            Kirigami.ShadowedImage {
                Layout.preferredWidth: 64
                Layout.preferredHeight: 64
                radius: 10
                source: tile.hasArt ? tile.media.art : ""
                visible: tile.hasArt
                fillMode: Image.PreserveAspectCrop
                sourceSize.width: 128
                sourceSize.height: 128
            }
            Rectangle {
                Layout.preferredWidth: 64
                Layout.preferredHeight: 64
                radius: 10
                visible: !tile.hasArt
                color: Qt.rgba(0.24, 0.73, 0.51, 0.18)
                Kirigami.Icon {
                    anchors.centerIn: parent
                    width: 28
                    height: 28
                    source: "media-default-album"
                    isMask: true
                    color: root.green
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    font.bold: true
                    font.pixelSize: Kirigami.Theme.defaultFont.pixelSize * 1.1
                    text: tile.media ? (tile.media.title || tile.media.app) : ""
                }
                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    opacity: 0.75
                    text: tile.media ? [tile.media.artist, tile.media.app].filter((x) => !!x).join(" · ") : ""
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 4
            PlasmaComponents.ToolButton {
                icon.name: "media-skip-backward"
                enabled: !!tile.media && tile.media.can_previous
                onClicked: root.run(["media", "previous"])
            }
            // The big round play/pause button.
            Rectangle {
                Layout.preferredWidth: 34
                Layout.preferredHeight: 34
                radius: 17
                color: playArea.pressed ? root.greenDark : root.green
                Kirigami.Icon {
                    anchors.centerIn: parent
                    width: 18
                    height: 18
                    isMask: true
                    color: "white"
                    source: tile.media && tile.media.playing ? "media-playback-pause" : "media-playback-start"
                }
                MouseArea {
                    id: playArea
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.run(["media", "play-pause"])
                }
            }
            PlasmaComponents.ToolButton {
                icon.name: "media-skip-forward"
                enabled: !!tile.media && tile.media.can_next
                onClicked: root.run(["media", "next"])
            }
            Kirigami.Icon {
                source: "audio-volume-medium"
                Layout.preferredWidth: 16
                Layout.preferredHeight: 16
                Layout.leftMargin: 6
                opacity: 0.8
            }
            PlasmaComponents.Slider {
                id: volume
                Layout.fillWidth: true
                from: 0
                to: tile.media ? Math.max(1, tile.media.max_volume) : 15
                stepSize: 1
                // Follow the phone unless you're dragging it.
                Binding on value {
                    when: !volume.pressed
                    value: tile.media ? tile.media.volume : 0
                }
                onPressedChanged: if (!pressed) root.run(["media", "volume", String(Math.round(value))])
            }
        }
    }
}
