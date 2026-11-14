# Shared by scripts/e2e-phones.sh and scripts/e2e-ios.sh (sourced, not run): the Mac side of
# the phone link end to end. A throwaway keychain, the e2e driver (`agentpulse --phone-e2e`,
# cargo feature e2e) with the link bridge built for it, and a local relay (`wrangler dev`).
#
# Keychain: every item goes into a throwaway keychain file under $TMPDIR (the bridge's
# AGENTPULSE_KEYCHAIN), never added to the search list. The login keychain must hold no
# pairing item before (else the check after would mean nothing) and after.
# Network: localhost only. wrangler's metrics and update check are off.
# Data: a temp directory, never ~/Library/Application Support/AgentPulse.
# A sourcing script may define `e2e_cleanup` for its own cleanup on exit.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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
    if declare -F e2e_cleanup >/dev/null; then e2e_cleanup; fi
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

step "Building the e2e driver and the link bridge"
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

# Prints a log without the relay key ($1: the key, may be empty; then the files).
show_log() {
    local key="$1"
    shift
    grep -v -e RELAY_KEY ${key:+-e "$key"} "$@" || true
}

# Starts the relay into $WRANGLER and waits for it. $1: an output directory, $2: its
# RELAY_KEY, empty for an open relay.
start_relay() {
    local out="$1" key="$2" i
    # The key goes to wrangler in a file, never on a command line; an empty file for an
    # open relay, so a .dev.vars in relay/ is never read either.
    (umask 077 && if [[ -n "$key" ]]; then echo "RELAY_KEY=$key"; fi >"$out/.dev.vars")
    ! curl -s -o /dev/null "$RELAY/v1/health" || die "something already answers on $RELAY"
    (
        cd relay
        WRANGLER_SEND_METRICS=false WRANGLER_HIDE_BANNER=true \
            exec npx wrangler dev --local --port "$PORT" --env-file "$out/.dev.vars"
    ) >"$out/wrangler.log" 2>&1 &
    WRANGLER=$!
    for i in $(seq 120); do
        curl -sf -o /dev/null "$RELAY/v1/health" && break
        kill -0 "$WRANGLER" 2>/dev/null || { show_log "$key" "$out/wrangler.log"; die "wrangler exited"; }
        sleep 0.5
    done
    curl -sf "$RELAY/v1/health" || { show_log "$key" "$out/wrangler.log"; die "the relay did not answer within 60 s"; }
    echo
}

# Starts the driver into $MAC, its output in $1/mac.out and mac.log, and waits for its LINK.
# $2: the relay key the Mac sends, may be empty. Extra environment comes from the caller.
start_mac() {
    local out="$1" key="$2" i
    mkdir -p "$out/data"
    AGENTPULSE_DATA_DIR="$out/data" AGENTPULSE_KEYCHAIN="$KC" AGENTPULSE_LINK_BRIDGE="$DYLIB" \
        AGENTPULSE_E2E_RELAY_KEY="$key" "$BIN" --phone-e2e "$RELAY" >"$out/mac.out" 2>"$out/mac.log" &
    MAC=$!
    for i in $(seq 300); do
        grep -q '^LINK ' "$out/mac.out" && break
        kill -0 "$MAC" 2>/dev/null || break
        sleep 0.1
    done
}

stop_relay() {
    kill_tree "$WRANGLER"
    wait "$WRANGLER" 2>/dev/null || true
    WRANGLER=""
}

# The keychain checks after a run.
check_keychains() {
    login_clean || die "the login keychain has an item for $SERVICE"
    outside_search_list
    echo "    the throwaway keychain stayed out of the search list"
    echo "    the login keychain has no item for $SERVICE"
}
