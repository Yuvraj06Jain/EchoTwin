// Album art blurred as a tile background. Needs Qt 6.5's QtQuick.Effects; it is loaded
// through a Loader, so a Plasma without it falls back to a plain tint instead of failing.
import QtQuick
import QtQuick.Effects

Item {
    id: backdrop

    property url source
    property real radius: 14

    Image {
        id: art
        anchors.fill: parent
        source: backdrop.source
        fillMode: Image.PreserveAspectCrop
        sourceSize.width: 128
        sourceSize.height: 128
        visible: false
    }
    Rectangle {
        id: mask
        anchors.fill: parent
        radius: backdrop.radius
        visible: false
        layer.enabled: true
    }
    MultiEffect {
        anchors.fill: parent
        source: art
        blurEnabled: true
        blur: 1.0
        blurMax: 48
        saturation: 0.15
        maskEnabled: true
        maskSource: mask
    }
}
