// Renders the widget with sample data for a look without Plasma: see render.sh.
import QtQuick
import QtQuick.Window
import "../../dev.phonelink.widget/contents/ui"

Window {
    id: win
    width: 1100; height: 700; visible: true; color: "#14181c"
    property string out: "OUT_PATH"

    Item {
        id: root
        readonly property color green: "#3dbb82"
        readonly property color grey: "#8a9096"
        readonly property color greenDark: "#23905f"
        readonly property color amber: "#d9a441"
        property bool expanded: false
        property bool serviceUp: true
        property string note: ""
        property real seenMs: 0
        property var st: ({
            paired: true, connection: true, battery_saver: true,
            phone: { id: "9C21", name: "Galaxy S24 FE", connected: true, via: "wifi", paused_by_peer: false, can_ring: true,
                     status: { battery: 82, charging: true, network: "5G", signal: 3 } },
            media: { app: "YouTube Music", title: "Tum Hi Ho", artist: "Arijit Singh", playing: true, can_previous: true, can_next: true,
                     volume: 9, max_volume: 15, art: Qt.resolvedUrl("art.png") },
            notifications: [
                { id: "a", app: "WhatsApp", title: "Rahul", text: "Are you coming to the lab today?", posted_ms: Date.now() - 20000 },
                { id: "b", app: "Messages", title: "AX-HDFCBK", text: "482913 is your OTP for login. Do not share it.", posted_ms: Date.now() - 240000, code: "482913" },
                { id: "c", app: "Gmail", title: "Placement Cell", text: "Interview schedule", posted_ms: Date.now() - 600000 } ],
            sms: [ { id: "1", name: "Mom", unread: 1, text: "Call me", date_ms: Date.now() }, { id: "2", name: "Rahul", unread: 1, text: "ok", date_ms: Date.now() }, { id: "3", name: "Airtel", unread: 0, text: "Recharge", date_ms: Date.now() } ],
            transfers: [ { id: "t", name: "notes-week-10.pdf", size: 1000, done: 640, state: "active", outgoing: true, rate: 12000000, eta_secs: 50 } ]
        })
        readonly property var phone: st.phone
        readonly property bool online: !!(phone && phone.connected)
        readonly property bool switchedOff: st.connection === false
        readonly property var status: phone && phone.status ? phone.status : null
        readonly property int newCount: 5
        readonly property int battery: status ? status.battery : -1
        readonly property var activeTransfer: st.transfers[0]
        function run(a) {}
        function flash(t) { note = t }
        function openApp(t) {}
        function sendUrls(u) {}
    }

    // A wallpaper-ish backdrop, the popup, and a panel with the icon.
    Rectangle { anchors.fill: parent; gradient: Gradient { GradientStop { position: 0; color: "#1f4d44" } GradientStop { position: 1; color: "#2b2140" } } }
    Rectangle {
        x: 40; y: 30; width: 29 * 18 + 16; height: 28 * 18 + 50
        radius: 10; color: Qt.rgba(0.11, 0.13, 0.15, 0.85); border.color: Qt.rgba(1,1,1,0.08)
        FullRepresentation { anchors.fill: parent }
    }
    Rectangle {
        x: 640; y: 60; width: 300; height: 44; radius: 6; color: Qt.rgba(0.11, 0.13, 0.15, 0.95)
        Row {
            anchors.centerIn: parent; spacing: 24
            Repeater {
                model: 3
                CompactRepresentation { width: 30; height: 44; scale: 1 }
            }
        }
    }
    Rectangle { x: 640; y: 130; width: 300; height: 200; color: Qt.rgba(0.11, 0.13, 0.15, 0.95); radius: 6
        CompactRepresentation { width: 30; height: 44; anchors.centerIn: parent; scale: 4 } }
    Timer { interval: 1500; running: true; onTriggered: win.contentItem.grabToImage(function (r) { r.saveToFile(win.out); Qt.quit(); }) }
}
