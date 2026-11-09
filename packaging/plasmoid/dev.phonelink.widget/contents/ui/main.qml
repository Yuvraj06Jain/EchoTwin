// EchoTwin panel widget (Plasma 6). Talks to phonelinkd through the `phonelink` CLI.
import QtQuick
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasma5support as P5Support

import "shell.js" as Shell

PlasmoidItem {
    id: root

    readonly property color green: "#3dbb82"
    readonly property color grey: "#8a9096"

    // Last state from `phonelink widget` (see desktop/daemon/src/bin/phonelink.rs).
    property var st: ({ paired: false, phone: null, media: null, notifications: [], sms: [], transfers: [] })
    property bool serviceUp: true
    property string note: ""
    // Newest notification time the user has seen in the popup (for the unread dot).
    property real seenMs: 0

    readonly property var phone: st.phone
    readonly property bool online: !!(phone && phone.connected)
    readonly property bool switchedOff: st.connection === false
    readonly property var status: phone && phone.status ? phone.status : null
    readonly property bool unread: {
        var n = st.notifications || [];
        for (var i = 0; i < n.length; i++) if (n[i].posted_ms > seenMs) return true;
        var s = st.sms || [];
        for (var j = 0; j < s.length; j++) if (s[j].unread > 0) return true;
        return false;
    }

    function statusText() {
        if (!serviceUp) return "EchoTwin isn't running";
        if (!st.paired) return "No phone paired";
        if (switchedOff) return phone.name + " · connection off";
        if (!online && phone.paused_by_peer) return phone.name + " · switched off on the phone";
        if (!online) return phone.name + " · offline";
        var parts = [phone.name];
        if (status && status.battery !== null && status.battery !== undefined)
            parts.push(status.battery + "%" + (status.charging ? " charging" : ""));
        if (status && status.network) parts.push(status.network);
        if (phone.via === "bluetooth") parts.push("Bluetooth");
        return parts.join(" · ");
    }

    toolTipMainText: "EchoTwin"
    toolTipSubText: note !== "" ? note : statusText()
    Plasmoid.icon: "smartphone"
    Plasmoid.status: online && unread ? PlasmaCore.Types.ActiveStatus : PlasmaCore.Types.PassiveStatus

    onExpandedChanged: if (expanded) { markSeen(); refresh(); }

    function markSeen() {
        var n = st.notifications || [];
        for (var i = 0; i < n.length; i++) seenMs = Math.max(seenMs, n[i].posted_ms);
    }

    // ---------------------------------------------------------------- running the CLI
    P5Support.DataSource {
        id: exec
        engine: "executable"
        connectedSources: []
        onNewData: (source, data) => {
            disconnectSource(source);
            if (source === root.pollSource) {
                // The waiting `widget` call answered: show it and wait for the next change.
                root.pollSource = "";
                if (root.apply(data)) {
                    // Never more than one call a second, whatever happens.
                    if (Date.now() - root.pollStarted < 1000)
                        nextTimer.start();
                    else
                        root.poll();
                } else {
                    root.lastHash = "";
                    retryTimer.start();
                }
                return;
            }
            if (source.indexOf(root.refreshCmd + " #") === 0) {
                root.apply(data);
                return;
            }
            var err = String(data["stderr"] || "").trim();
            if (data["exit code"] !== 0 && err !== "") root.flash(err.replace(/^Error: /, ""));
            root.refresh();
        }
    }

    readonly property string refreshCmd: Shell.command(["widget"])
    property int seq: 0
    // Instead of polling, one `phonelink widget --wait <hash>` is always running: it
    // prints when something changes (or after a minute), and the next one starts at once.
    property string lastHash: ""
    property string pollSource: ""
    property real pollStarted: 0

    // Show a `widget` result; false if the service is down or the output is unusable.
    function apply(data) {
        if (data["exit code"] !== 0) {
            serviceUp = false;
            return false;
        }
        try {
            st = JSON.parse(data["stdout"]);
        } catch (e) {
            serviceUp = false;
            return false;
        }
        lastHash = st.hash || "";
        serviceUp = true;
        if (expanded) markSeen();
        return true;
    }

    function poll() {
        seq += 1;
        var args = lastHash === "" ? ["widget"] : ["widget", "--wait", lastHash];
        // The counter keeps the engine from merging this with an earlier, identical call.
        pollSource = Shell.command(args) + " #" + seq;
        pollStarted = Date.now();
        exec.connectSource(pollSource);
    }

    // Read the state once now (popup opened, after an action); the waiting call goes on.
    function refresh() {
        seq += 1;
        exec.connectSource(refreshCmd + " #" + seq);
    }

    // Run `phonelink <args…>`; the counter keeps identical commands from being merged.
    function run(args) {
        seq += 1;
        exec.connectSource(Shell.command(args) + " #" + seq);
    }

    function flash(text) {
        note = text;
        noteTimer.restart();
    }

    function sendUrls(urls) {
        var paths = [];
        for (var i = 0; i < urls.length; i++) {
            var p = Shell.localPath(urls[i]);
            if (p) paths.push(p);
        }
        if (paths.length === 0) { flash("Only files on this computer can be sent"); return; }
        run(["send-file", "--notify"].concat(paths));
        flash(paths.length === 1 ? "Queued 1 file for your phone" : "Queued " + paths.length + " files for your phone");
    }

    function openApp(tab) {
        seq += 1;
        var args = tab ? " --show=" + Shell.quote(tab) : "";
        exec.connectSource('A="$HOME/.local/bin/phonelink-app"; [ -x "$A" ] || A=phonelink-app; setsid -f "$A"' + args + " >/dev/null 2>&1 #" + seq);
        root.expanded = false;
    }

    Timer { id: noteTimer; interval: 4000; onTriggered: root.note = "" }
    // The service was down (or the call failed): try again in a while.
    Timer { id: retryTimer; interval: 10000; onTriggered: root.poll() }
    Timer { id: nextTimer; interval: 1000; onTriggered: root.poll() }

    Component.onCompleted: poll()

    compactRepresentation: CompactRepresentation {}
    fullRepresentation: FullRepresentation {}
}
