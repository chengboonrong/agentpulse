#!/usr/bin/env bash
# End-to-end test of the Android app (android/app) on a booted emulator: the Mac side as
# in scripts/e2e-phones.sh (scripts/e2e-common.sh: throwaway keychain, the e2e driver, a
# local relay) with the direct connection off (AGENTPULSE_E2E_DIRECT=0, the app has none
# yet), and the real app, debug build, its secrets under the emulator's Android Keystore.
# `adb reverse` gives the emulator the relay at the same http://localhost:<port>. The link
# goes in as the debug-only extra `e2eLink` and `e2eAutoConfirm true` answers "Pair with
# <Mac>?"; the driver confirms the code and publishes, and says SEEN once the app has
# sent `seen`. Then the app is relaunched with the debug-only `e2eRemove true`, which
# removes the Mac: the app posts `unpair`, the Mac deletes the pairing at the relay and
# the driver ends OK.
#
#   scripts/e2e-android-app.sh                              the relay on port 8787
#   AGENTPULSE_E2E_PORT=8797 scripts/e2e-android-app.sh     another port
#   ANDROID_SERIAL=emulator-5556                            which emulator (default: the first)
#   AGENTPULSE_E2E_SHOT=<png>                               the screenshot (default target/e2e-android-app.png)
#
# Needs ANDROID_HOME (adb) and a JDK 17 for Gradle, besides what scripts/e2e-common.sh needs.
set -euo pipefail

source "$(dirname "$0")/e2e-common.sh"
export AGENTPULSE_E2E_DIRECT=0

ADB="${ANDROID_HOME:?set ANDROID_HOME}/platform-tools/adb"
PKG="io.github.chengboonrong.agentpulse"
SHOT="${AGENTPULSE_E2E_SHOT:-$TARGET/e2e-android-app.png}"
export ANDROID_SERIAL="${ANDROID_SERIAL:-$("$ADB" devices | awk '/^emulator-[0-9]+\tdevice$/ { print $1; exit }')}"
[[ -n "$ANDROID_SERIAL" ]] || die "no booted emulator (adb devices)"
e2e_cleanup() {
    "$ADB" reverse --remove "tcp:$PORT" 2>/dev/null || true
    "$ADB" uninstall "$PKG" >/dev/null 2>&1 || true
}

step "Building and installing the app (debug) on $ANDROID_SERIAL"
(cd android && ./gradlew -q --console=plain :app:assembleDebug)
"$ADB" install -r android/app/build/outputs/apk/debug/app-debug.apk >/dev/null
"$ADB" shell pm clear "$PKG" >/dev/null
"$ADB" reverse "tcp:$PORT" "tcp:$PORT" >/dev/null

out="$WORK/android-app"
mkdir -p "$out"
step "Starting the relay"
start_relay "$out" ""

step "Pairing"
start_mac "$out" ""
LINK="$(sed -n 's/^LINK //p' "$out/mac.out" | head -n1)"
[[ -n "$LINK" ]] || { cat "$out/mac.out" "$out/mac.log"; die "the Mac printed no link"; }
# The link has `&`s, which `adb shell` hands to the device's shell: quote it there.
"$ADB" shell "am start -S -W -n $PKG/.MainActivity -e e2eAutoConfirm true -e e2eLink '$LINK'" >/dev/null
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
    echo "--- app log"
    "$ADB" logcat -d --pid="$("$ADB" shell pidof "$PKG" | tr -d '\r')" 2>/dev/null | tail -n 40 || true
    die "$1"
}
grep -q '^SEEN$' "$out/mac.out" || failed "the app did not pair and send seen"
echo "    the app paired and sent seen"

step "Screenshot of the Mac view"
sleep 3
mkdir -p "$(dirname "$SHOT")"
"$ADB" exec-out screencap -p >"$SHOT"
echo "    $SHOT"

step "Removing the Mac in the app"
"$ADB" shell am start -S -W -n "$PKG/.MainActivity" -e e2eRemove true >/dev/null
mac=0
wait "$MAC" || mac=$?
MAC=""
[[ "$mac" == 0 && "$(tail -n1 "$out/mac.out")" == OK ]] || failed "the Mac did not see the app's unpair (exit $mac)"
echo "--- Mac (agentpulse --phone-e2e), exit $mac"
sed 's/^LINK .*/LINK <the pairing link, one-time secret elided>/' "$out/mac.out"
stop_relay
echo "    relay requests:"
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
