#!/usr/bin/env bash
# Regenerates macos/dmg/DS_Store, the DMG window layout that scripts/bundle.sh copies onto
# every image. Run once by hand, from the repo root, after changing the window size, the icon
# positions or the item names; it drives Finder, so macOS asks once for Automation permission.
# Positions and the window size must match macos/dmg/render-background.swift.
set -euo pipefail
cd "$(dirname "$0")/../.."

WORK="$(mktemp -d)"
# The volume name must be the one bundle.sh uses: Finder finds the background through an
# alias that records it.
VOLNAME="AgentPulse"
MNT="/Volumes/$VOLNAME"   # Finder only addresses volumes it can see, so mount it there
trap 'hdiutil detach -quiet -force "$MNT" 2>/dev/null || true; rm -rf "$WORK"' EXIT

# Item names are all Finder stores, so an empty folder stands in for the app.
mkdir -p "$WORK/stage/AgentPulse.app" "$WORK/stage/.background"
ln -s /Applications "$WORK/stage/Applications"
tiffutil -cathidpicheck macos/dmg/background.png macos/dmg/background@2x.png \
    -out "$WORK/stage/.background/background.tiff" 2>/dev/null
hdiutil create -quiet -volname "$VOLNAME" -srcfolder "$WORK/stage" -fs HFS+ -format UDRW "$WORK/rw.dmg"
[[ -e "$MNT" ]] && { echo "$MNT is already mounted; eject it first" >&2; exit 1; }
hdiutil attach -quiet "$WORK/rw.dmg"

osascript <<EOF
tell application "Finder"
    set d to disk "$VOLNAME"
    open d
    set w to container window of d
    set current view of w to icon view
    set toolbar visible of w to false
    set statusbar visible of w to false
    set sidebar width of w to 0
    -- {left, top, right, bottom}: a 660x420 content area.
    set bounds of w to {200, 120, 860, 540}
    set o to icon view options of w
    set arrangement of o to not arranged
    set icon size of o to 128
    set text size of o to 13
    set label position of o to bottom
    set shows item info of o to false
    set shows icon preview of o to false
    set background picture of o to file ".background:background.tiff" of d
    set position of item "AgentPulse.app" of d to {170, 190}
    set position of item "Applications" of d to {490, 190}
    update d without registering applications
    delay 1
    close w
end tell
EOF

# Finder writes .DS_Store lazily; wait for it.
for _ in $(seq 20); do [[ -s "$MNT/.DS_Store" ]] && break; sleep 0.5; done
sync
cp "$MNT/.DS_Store" macos/dmg/DS_Store
echo "Wrote macos/dmg/DS_Store ($(wc -c < macos/dmg/DS_Store | tr -d ' ') bytes)"
