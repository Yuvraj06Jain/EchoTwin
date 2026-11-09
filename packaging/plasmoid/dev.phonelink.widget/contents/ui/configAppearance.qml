// Right-click the widget → Configure EchoTwin… → Appearance.
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM
import org.kde.plasma.plasma5support as P5Support

KCM.SimpleKCM {
    property alias cfg_backgroundOpacity: opacitySlider.value
    property int cfg_backgroundOpacityDefault: 75

    P5Support.DataSource {
        id: exec
        engine: "executable"
        onNewData: (source) => disconnectSource(source)
    }

    Kirigami.FormLayout {
        RowLayout {
            Kirigami.FormData.label: "Background opacity:"
            QQC2.Slider {
                id: opacitySlider
                from: 30
                to: 100
                stepSize: 5
                Layout.preferredWidth: Kirigami.Units.gridUnit * 12
            }
            QQC2.Label { text: Math.round(opacitySlider.value) + "%" }
        }
        QQC2.Button {
            Kirigami.FormData.label: "Blur:"
            icon.name: "preferences-desktop-effects"
            text: "Open KDE blur settings…"
            onClicked: exec.connectSource("kcmshell6 kcm_kwin_effects >/dev/null 2>&1 || systemsettings kcm_kwin_effects >/dev/null 2>&1 &")
        }
        QQC2.Label {
            Layout.maximumWidth: Kirigami.Units.gridUnit * 20
            wrapMode: Text.Wrap
            opacity: 0.7
            text: "KDE blurs behind the popup when its Blur effect is on. Its strength (and turning it off) is set there, for all windows."
        }
    }
}
