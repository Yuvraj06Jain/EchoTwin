import QtQuick
// Stand-in: a glyph per icon name.
Item {
    id: i
    property var source
    property bool isMask: false
    property color color: Theme.textColor
    implicitWidth: 16; implicitHeight: 16
    readonly property var glyphs: ({ "media-skip-backward": "⏮", "media-skip-forward": "⏭", "media-playback-start": "▶", "media-playback-pause": "❚❚",
        "media-playback-stop": "■", "notifications": "🔔", "go-next": "›", "go-previous": "‹", "document-send": "⇪", "view-list-details": "☰",
        "window-new": "⧉", "system-shutdown": "⏻", "view-refresh": "⟳", "document-edit": "✎", "battery-profile-powersave": "☘", "audio-volume-medium": "🔈",
        "media-default-album": "♪", "smartphone": "▯", "mail-message": "✉", "window-close": "×", "edit-copy": "⎘", "dialog-cancel": "⊘" })
    Text { anchors.centerIn: parent; text: i.glyphs[String(i.source)] || "•"; color: i.color; font.pixelSize: Math.min(i.width, i.height) * 0.9 }
}
