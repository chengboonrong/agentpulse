#!/usr/bin/env bash
# End-to-end test of the Android port of the phone link (android/link) through a local
# relay and the direct connection: the Mac side as in scripts/e2e-phones.sh
# (scripts/e2e-common.sh: throwaway keychain, the e2e driver, a local relay), and
# `./gradlew :link:phoneSim` as the phone. The phone pairs, the codes must match, it fetches
# the Mac's state, sends `seen`, connects directly (finding the Mac with `dns-sd`) and reads
# its first frame, then removes the pairing; the Mac must see it seen and connected
# directly, then the `unpair`, and delete the pairing at the relay.
#
#   scripts/e2e-android.sh                              the relay on port 8787
#   AGENTPULSE_E2E_PORT=8797 scripts/e2e-android.sh     another port
#
# Needs a JDK 17 for Gradle, besides what scripts/e2e-common.sh needs.
set -euo pipefail

source "$(dirname "$0")/e2e-common.sh"

step "Building the Android phone simulator"
(cd android && ./gradlew -q --console=plain :link:testClasses)

out="$WORK/android"
mkdir -p "$out"
step "Starting the relay"
start_relay "$out" ""

step "Pairing"
start_mac "$out" ""
phone=0 mac=0
grep -m1 '^LINK ' "$out/mac.out" |
    (cd android && ./gradlew -q --console=plain :link:phoneSim) >"$out/phone.out" 2>"$out/phone.log" || phone=$?
wait "$MAC" || mac=$?
MAC=""
stop_relay

echo "--- Mac (agentpulse --phone-e2e), exit $mac"
sed 's/^LINK .*/LINK <the pairing link, one-time secret elided>/' "$out/mac.out"
echo "--- phone (:link:phoneSim), exit $phone"
cat "$out/phone.out" "$out/phone.log"
mac_code="$(sed -n 's/^CODE //p' "$out/mac.out")"
phone_code="$(sed -n 's/^CODE //p' "$out/phone.out")"
if [[ -z "$mac_code" || "$mac_code" != "$phone_code" || "$phone" != 0 || "$(tail -n1 "$out/mac.out")" != OK ]] ||
    ! grep -q '^DIRECT state seq ' "$out/phone.out" || ! grep -q '^SEEN$' "$out/mac.out"; then
    echo "--- Mac log"
    cat "$out/mac.log"
    echo "--- relay log"
    show_log "" "$out/wrangler.log" | tail -n 40
    die "the Android phone and the Mac did not pair, connect directly and unpair"
fi
echo "    codes match ($mac_code); the phone read the Mac's first direct frame; the Mac saw it"
echo "    seen and connected directly, then the unpair, and ended OK"
echo "    relay requests:"
grep -oE '(POST|GET|PUT|DELETE) /v1/pairings/[^/ ]+(/(hello|down|up))? [0-9]+' "$out/wrangler.log" |
    sed -E 's|/v1/pairings/[^/ ]+|/v1/pairings/<id>|' | sort | uniq -c ||
    die "no relay requests in the log"
grep -qE 'DELETE /v1/pairings/[^/ ]+ 204' "$out/wrangler.log" || die "the pairing was not deleted at the relay"

step "Checking the keychains"
check_keychains
echo
echo "PASS"
