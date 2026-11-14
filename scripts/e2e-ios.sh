#!/usr/bin/env bash
# End-to-end test of the iPhone app (ios/) in the simulator: the Mac side as in
# scripts/e2e-phones.sh (scripts/e2e-common.sh: throwaway keychain, the e2e driver, a local
# relay) and the real app, Debug build, in a simulator of its own that is deleted after.
# The driver's link reaches the app as a DEBUG-only launch argument, `-AgentPulseE2ELink`,
# into the same path as an opened link: `simctl openurl` makes the simulator ask "Open in
# AgentPulse?", which needs a tap. The DEBUG-only `-AgentPulseE2EAutoConfirm YES` answers
# the app's own "Pair with <Mac>?" for the same reason. The driver confirms the code,
# publishes, and says SEEN once the app has sent `seen` and is connected directly; it keeps
# publishing (a second state 10 s after the first) while the app's Mac view is
# screenshotted. Then the app is relaunched with the DEBUG-only `-AgentPulseE2ERemove YES`,
# which removes the Mac: the app posts `unpair`, the Mac deletes the pairing at the relay
# and the driver ends OK.
#
#   scripts/e2e-ios.sh                              the relay on port 8787
#   AGENTPULSE_E2E_PORT=8797 scripts/e2e-ios.sh     another port
#   AGENTPULSE_E2E_SHOT=<png>                       the screenshot (default target/e2e-ios.png)
#   AGENTPULSE_E2E_DEVICE=<simctl device type id>   default iPhone 18 Pro
#
# The simulator shares this Mac's network, so the direct connection is simulator to the
# driver's listener over Bonjour on this Mac.
set -euo pipefail

source "$(dirname "$0")/e2e-common.sh"

BUNDLE="io.github.chengboonrong.AgentPulse.phone"
DEVICE="${AGENTPULSE_E2E_DEVICE:-com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro}"
SHOT="${AGENTPULSE_E2E_SHOT:-$TARGET/e2e-ios.png}"
SIM=""
e2e_cleanup() {
    if [[ -n "$SIM" ]]; then
        xcrun simctl shutdown "$SIM" 2>/dev/null || true
        xcrun simctl delete "$SIM" 2>/dev/null || true
    fi
}

step "Creating a simulator"
RUNTIME="$(xcrun simctl list runtimes | awk '/^iOS / && !/unavailable/ { r = $NF } END { print r }')"
[[ -n "$RUNTIME" ]] || die "no iOS simulator runtime"
SIM="$(xcrun simctl create "AgentPulse E2E" "$DEVICE" "$RUNTIME")"
echo "    $SIM ($DEVICE, $RUNTIME)"
xcrun simctl boot "$SIM"
xcrun simctl bootstatus "$SIM" -b >/dev/null

step "Building and installing the app (Debug)"
xcodegen generate --quiet --spec ios/project.yml --project ios
xcodebuild -quiet -project ios/AgentPulsePhone.xcodeproj -scheme AgentPulse -configuration Debug \
    -destination "platform=iOS Simulator,id=$SIM" -derivedDataPath "$TARGET/ios-e2e" build
xcrun simctl install "$SIM" "$TARGET/ios-e2e/Build/Products/Debug-iphonesimulator/AgentPulse.app"

out="$WORK/ios"
mkdir -p "$out"
step "Starting the relay"
start_relay "$out" ""

step "Pairing"
start_mac "$out" ""
LINK="$(sed -n 's/^LINK //p' "$out/mac.out" | head -n1)"
[[ -n "$LINK" ]] || { cat "$out/mac.out" "$out/mac.log"; die "the Mac printed no link"; }
xcrun simctl launch "$SIM" "$BUNDLE" -AgentPulseE2EAutoConfirm YES -AgentPulseE2ELink "$LINK" >/dev/null
# The driver gives up after 90 s.
for i in $(seq 1000); do
    grep -q '^SEEN$' "$out/mac.out" && break
    kill -0 "$MAC" 2>/dev/null || break
    sleep 0.1
done
failed() {
    echo "--- Mac (agentpulse --phone-e2e)"
    sed 's/^LINK .*/LINK <the pairing link, one-time secret elided>/' "$out/mac.out"
    echo "--- Mac log"
    cat "$out/mac.log"
    echo "--- relay log"
    show_log "" "$out/wrangler.log" | tail -n 40
    die "$1"
}
grep -q '^SEEN$' "$out/mac.out" || failed "the app did not pair, send seen and connect directly"
echo "    the app paired, sent seen and connected directly"

step "Screenshot of the Mac view, after the second state"
sleep 15
xcrun simctl io "$SIM" screenshot "$SHOT" >/dev/null
echo "    $SHOT"

step "Removing the Mac in the app"
xcrun simctl launch --terminate-running-process "$SIM" "$BUNDLE" -AgentPulseE2ERemove YES >/dev/null
mac=0
wait "$MAC" || mac=$?
MAC=""
[[ "$mac" == 0 && "$(tail -n1 "$out/mac.out")" == OK ]] || failed "the Mac did not see the app's unpair (exit $mac)"
echo "--- Mac (agentpulse --phone-e2e), exit $mac"
sed 's/^LINK .*/LINK <the pairing link, one-time secret elided>/' "$out/mac.out"
stop_relay
echo "    relay requests (the phone: hello, GET down, POST up, DELETE only if its unpair failed;"
echo "    the Mac: PUT down, GET up, DELETE on the unpair):"
grep -oE '(POST|GET|PUT|DELETE) /v1/pairings/[^/ ]+(/(hello|down|up))? [0-9]+' "$out/wrangler.log" |
    sed -E 's|/v1/pairings/[^/ ]+|/v1/pairings/<id>|' | sort | uniq -c ||
    die "no relay requests in the log"
grep -qE 'DELETE /v1/pairings/[^/ ]+ 204' "$out/wrangler.log" || die "the pairing was not deleted at the relay"
[[ "$(grep -cE 'POST /v1/pairings/[^/ ]+/up 204' "$out/wrangler.log")" -ge 2 ]] ||
    die "the app's unpair did not reach the relay"
echo "    the app's unpair went through and the relay pairing was deleted"

step "Checking the keychains"
check_keychains
echo
echo "PASS"
