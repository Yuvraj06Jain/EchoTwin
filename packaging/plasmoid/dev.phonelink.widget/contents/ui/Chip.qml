// An app icon, or a coloured letter chip when the app's icon isn't known.
import QtQuick
import org.kde.plasma.components as PlasmaComponents

import "shell.js" as Shell

Rectangle {
    id: chip

    property string name
    property string icon: ""
    property bool round: false

    width: 26
    height: 26
    radius: round ? width / 2 : 7
    color: img.status === Image.Ready ? "transparent" : Shell.colorFor(name)

    Image {
        id: img
        anchors.fill: parent
        source: chip.icon ? "file://" + encodeURI(chip.icon) : ""
        sourceSize.width: 64
        sourceSize.height: 64
        visible: status === Image.Ready
        smooth: true
    }
    PlasmaComponents.Label {
        anchors.centerIn: parent
        visible: img.status !== Image.Ready
        text: Shell.initial(chip.name)
        color: "white"
        font.bold: true
        font.pixelSize: chip.height * 0.48
    }
}
