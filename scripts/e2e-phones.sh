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
# Keychain: every item goes into a throwaway keychain file under $TMPDIR (the bridge's
# AGENTPULSE_KEYCHAIN), never added to the search list. The login keychain must hold no
# pairing item before (else the check after would mean nothing) and after.
# Network: localhost only. wrangler's metrics and update check are off. The direct
# connection uses Bonjour on this Mac, which may need the Local Network permission.
# Data: a temp directory, never ~/Library/Application Support/AgentPulse.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SERVICE="io.github.chengboonrong.AgentPulse.pairing"
PORT="${AGENTPULSE_E2E_PORT:-8787}"
RELAY="http://localhost:$PORT"
LOGIN="$HOME/Library/Keychains/login.keychain-db"

die() { echo "error: $*" >&2; exit 1; }
step() { echo "==> $*"; }

for tool in cargo swift xcodebuild xcodegen security npx curl openssl plutil; do
    command -v "$tool" >/dev/null || die "$tool not found"
done

login_clean() { ! security find-generic-password -s "$SERVICE" "$LOGIN" >/dev/null 2>&1; }
login_clean || die "the login keychain already has an item for $SERVICE, so this test could not tell whether it wrote one"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/agentpulse-e2e.XXXXXX")"
KC="$WORK/e2e.keychain-db"
WRANGLER=""
MAC=""
kill_tree() {
    local child
    for child in $(pgrep -P "$1"); do kill_tree "$child"; done
    kill "$1" 2>/dev/null || true
}
cleanup() {
    [[ -z "$MAC" ]] || kill_tree "$MAC"
    [[ -z "$WRANGLER" ]] || kill_tree "$WRANGLER"
    security delete-keychain "$KC" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

step "Making a throwaway keychain"
PASSWORD="$(openssl rand -hex 24)"
security create-keychain -p "$PASSWORD" "$KC"
security unlock-keychain -p "$PASSWORD" "$KC"
security set-keychain-settings "$KC" # no auto-lock
outside_search_list() {
    if security list-keychains -d user | grep -qF "$KC" || security default-keychain -d user | grep -qF "$KC"; then
        die "the throwaway keychain got into the search list or became the default"
    fi
}
outside_search_list

step "Building the e2e driver, the link bridge and the phone simulator"
cargo build -p agentpulse-app --features e2e
TARGET="$(cargo metadata --format-version 1 --no-deps | plutil -extract target_directory raw -o - -)"
BIN="$TARGET/debug/agentpulse"
xcodegen generate --quiet --spec macos/project.yml --project macos
# AGENTPULSE_E2E compiles in the AGENTPULSE_KEYCHAIN override, which bundle.sh's build
# lacks; built apart from bundle.sh's products so the two never mix.
xcodebuild -quiet -project macos/AgentPulseWidgets.xcodeproj -target AgentPulseLinkBridge \
    -configuration Release SYMROOT="$TARGET/xcode-e2e/build" OBJROOT="$TARGET/xcode-e2e/obj" ARCHS="$(uname -m)" \
    SWIFT_ACTIVE_COMPILATION_CONDITIONS='$(inherited) AGENTPULSE_E2E' build
DYLIB="$TARGET/xcode-e2e/build/Release/libAgentPulseLinkBridge.dylib"
swift build --package-path link --product phone-sim

# $1: the relay's RELAY_KEY, empty for an open relay.
run_once() {
    local key="$1" name="${1:+relay-key}"
    name="${name:-open}"
    local out="$WORK/$name"
    mkdir -p "$out/data"
    # The key goes to wrangler in a file, never on a command line; an empty file for an
    # open relay, so a .dev.vars in relay/ is never read either.
    (umask 077 && if [[ -n "$key" ]]; then echo "RELAY_KEY=$key"; fi >"$out/.dev.vars")
    # Printed logs never show the key.
    show_log() { grep -v -e RELAY_KEY ${key:+-e "$key"} "$@" || true; }

    step "Run '$name': starting the relay"
    ! curl -s -o /dev/null "$RELAY/v1/health" || die "something already answers on $RELAY"
    (
        cd relay
        WRANGLER_SEND_METRICS=false WRANGLER_HIDE_BANNER=true \
            exec npx wrangler dev --local --port "$PORT" --env-file "$out/.dev.vars"
    ) >"$out/wrangler.log" 2>&1 &
    WRANGLER=$!
    local i
    for i in $(seq 120); do
        curl -sf -o /dev/null "$RELAY/v1/health" && break
        kill -0 "$WRANGLER" 2>/dev/null || { show_log "$out/wrangler.log"; die "wrangler exited"; }
        sleep 0.5
    done
    curl -sf "$RELAY/v1/health" || { show_log "$out/wrangler.log"; die "the relay did not answer within 60 s"; }
    echo
    if [[ -n "$key" ]]; then
        local refused
        refused="$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d '{}' "$RELAY/v1/pairings")"
        [[ "$refused" == 401 ]] || die "a create without RELAY_KEY got HTTP $refused, not 401"
        echo "    a create without the key: HTTP 401"
    fi

    step "Run '$name': pairing"
    AGENTPULSE_DATA_DIR="$out/data" AGENTPULSE_KEYCHAIN="$KC" AGENTPULSE_LINK_BRIDGE="$DYLIB" \
        AGENTPULSE_E2E_RELAY_KEY="$key" "$BIN" --phone-e2e "$RELAY" >"$out/mac.out" 2>"$out/mac.log" &
    MAC=$!
    for i in $(seq 300); do
        grep -q '^LINK ' "$out/mac.out" && break
        kill -0 "$MAC" 2>/dev/null || break
        sleep 0.1
    done
    local phone=0 mac=0
    grep -m1 '^LINK ' "$out/mac.out" |
        swift run --skip-build --package-path link phone-sim >"$out/phone.out" 2>"$out/phone.log" || phone=$?
    wait "$MAC" || mac=$?
    MAC=""
    kill_tree "$WRANGLER"
    wait "$WRANGLER" 2>/dev/null || true
    WRANGLER=""

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
        show_log "$out/wrangler.log" | tail -n 40
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
login_clean || die "the login keychain has an item for $SERVICE"
outside_search_list
echo "    the throwaway keychain stayed out of the search list"
echo "    the login keychain has no item for $SERVICE"
echo
echo "PASS"
