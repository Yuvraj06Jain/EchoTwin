#!/bin/sh
# Render the panel widget without Plasma, for a quick look at layout changes:
#   packaging/plasmoid/tests/preview/render.sh /tmp/widget.png
# Needs qmlscene (Debian/Ubuntu: qmlscene-qt6 qml6-module-qtquick-shapes
# qml6-module-qtquick-controls qml6-module-qtquick-dialogs qml6-module-qtquick-layouts).
# `stubs/` stands in for Plasma's and Kirigami's QML modules: only layout and our own
# drawing are real. Blurred album art needs Qt 6.5, so here you see the fallback tint.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-/tmp/echotwin-widget.png}
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
tmp="$here/.harness-run.qml"
trap 'rm -f "$tmp"' EXIT
sed "s|OUT_PATH|$out|" "$here/harness.qml" > "$tmp"
rm -f "$out"
QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software QT_QUICK_CONTROLS_STYLE=Basic \
  timeout 60 /usr/lib/qt6/bin/qmlscene -I "$here/stubs" "$tmp" 2>&1 |
  grep -v 'deprecated\|XDG_RUNTIME\|QtQuick.Effects' || true
[ -s "$out" ] && echo "wrote $out"
