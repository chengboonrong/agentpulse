#!/usr/bin/env bash
# End-to-end test of the phone link (docs/mobile.md): the real link bridge dylib and Rust
# worker (`agentpulse --phone-e2e`, cargo feature e2e), a local relay (`wrangler dev`) and
# the phone simulator (link/, `phone-sim`) pair, confirm, publish, send `seen` through the
# relay and the direct connection, and unpair. Two runs: an open relay, then one with
# RELAY_KEY set, which the Mac must send to create the pairing.
#
#   scripts/e2e-phones.sh                         the relay on port 8787
#   AGENTPULSE_E2E_PORT=8797 scripts/e2e-phones.sh    another port
#
# Keychain, network and data: see scripts/e2e-common.sh. The direct connection uses
# Bonjour on this Mac, which may need the Local Network permission.
set -euo pipefail

source "$(dirname "$0")/e2e-common.sh"

step "Building the phone simulator"
swift build --package-path link --product phone-sim

# $1: the relay's RELAY_KEY, empty for an open relay.
run_once() {
    local key="$1" name="${1:+relay-key}"
    name="${name:-open}"
    local out="$WORK/$name"
    mkdir -p "$out"

    step "Run '$name': starting the relay"
    start_relay "$out" "$key"
    if [[ -n "$key" ]]; then
        local refused
        refused="$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d '{}' "$RELAY/v1/pairings")"
        [[ "$refused" == 401 ]] || die "a create without RELAY_KEY got HTTP $refused, not 401"
        echo "    a create without the key: HTTP 401"
    fi

    step "Run '$name': pairing"
    start_mac "$out" "$key"
    local phone=0 mac=0
    grep -m1 '^LINK ' "$out/mac.out" |
        swift run --skip-build --package-path link phone-sim >"$out/phone.out" 2>"$out/phone.log" || phone=$?
    wait "$MAC" || mac=$?
    MAC=""
    stop_relay

    echo "--- Mac (agentpulse --phone-e2e), exit $mac"
    sed 's/^LINK .*/LINK <the pairing link, one-time secret elided>/' "$out/mac.out"
    echo "--- phone (phone-sim), exit $phone"
    cat "$out/phone.out" "$out/phone.log"
    local mac_code phone_code
    mac_code="$(sed -n 's/^CODE //p' "$out/mac.out")"
    phone_code="$(sed -n 's/^CODE //p' "$out/phone.out")"
    if [[ -z "$mac_code" || "$mac_code" != "$phone_code" || "$phone" != 0 || "$(tail -n1 "$out/mac.out")" != OK ]]; then
        echo "--- Mac log"
        cat "$out/mac.log"
        echo "--- relay log"
        show_log "$key" "$out/wrangler.log" | tail -n 40
        die "run '$name' failed"
    fi
    echo "    codes match ($mac_code); the Mac ended OK"
}

run_once ""
run_once "$(openssl rand -hex 16)"

step "Checking the keychains"
security find-generic-password -s "$SERVICE" -a relay-key "$KC" >/dev/null ||
    die "the relay key is not in the throwaway keychain"
echo "    the relay key went into the throwaway keychain"
check_keychains
echo
echo "PASS"
