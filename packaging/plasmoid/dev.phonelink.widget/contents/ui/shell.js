.pragma library

// Everything the widget runs goes through /bin/sh, so every argument is single-quoted:
// a file called  $(rm -rf ~).txt  stays just a name.
function quote(s) {
    return "'" + String(s).replace(/'/g, "'\\''") + "'";
}

// The phonelink CLI from install.sh (~/.local/bin is often not on the panel's PATH).
var CLI = 'PL="$HOME/.local/bin/phonelink"; [ -x "$PL" ] || PL=phonelink; "$PL"';

function command(args) {
    return CLI + " " + args.map(quote).join(" ");
}

// file:///home/me/My%20File.pdf -> /home/me/My File.pdf (other URLs: null).
function localPath(url) {
    var u = String(url);
    if (u.indexOf("file://") !== 0) return null;
    try {
        return decodeURIComponent(u.substring(7));
    } catch (e) {
        return null;
    }
}

function percent(done, size) {
    return size > 0 ? Math.min(100, Math.floor(done * 100 / size)) : 100;
}

function ago(ms) {
    if (!ms) return "";
    var s = Math.max(0, (Date.now() - ms) / 1000);
    if (s < 60) return "now";
    if (s < 3600) return Math.floor(s / 60) + " min";
    if (s < 86400) return Math.floor(s / 3600) + " h";
    return Math.floor(s / 86400) + " d";
}

// " · 1 min left" (empty while the speed is unknown).
function left(secs, rate) {
    if (!rate || secs === null || secs === undefined) return "";
    if (secs < 60) return " · " + Math.max(1, Math.round(secs)) + " s left";
    if (secs < 3600) return " · " + Math.round(secs / 60) + " min left";
    return " · " + Math.floor(secs / 3600) + " h " + Math.round((secs % 3600) / 60) + " min left";
}
