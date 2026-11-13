#!/usr/bin/env bash
# Builds <cargo target dir>/bundle/AgentPulse.app (target/ unless CARGO_TARGET_DIR
# or build.target-dir says otherwise) with its WidgetKit extension and signs it.
#
# Team id: AGENTPULSE_TEAM_ID, else AGENTPULSE_TEAM_ID=... in macos/signing.local
# (untracked). The signing identity is the valid code-signing certificate
# whose OU is that team; it is selected by hash, never by name.
#
# Data path: the app writes ~/Library/Application Support/AgentPulse/Widget/
# widget.json and the sandboxed extension reads it through a read-only
# temporary-exception entitlement. An app group container does not work for an
# unsandboxed app signed without a provisioning profile: macOS denies it
# (TCC SystemPolicyAppDataDetailed, no prompt). See macos/README.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_ID="io.github.chengboonrong.AgentPulse"
WIDGET_ID="$APP_ID.Widgets"
SIGN_TIMEOUT=120

die() { echo "error: $*" >&2; exit 1; }
step() { echo "==> $*"; }
# macOS has no timeout(1); perl's alarm is enough to stop a codesign stuck on a keychain dialog.
with_timeout() { perl -e 'alarm shift; exec @ARGV or die "exec $ARGV[0]: $!"' "$@"; }

for tool in cargo xcodebuild xcodegen codesign security openssl plutil; do
    command -v "$tool" >/dev/null || die "$tool not found (Rust: rustup.rs; xcodegen: brew install xcodegen; the rest ship with Xcode)"
done

# --- team id and identity -------------------------------------------------
TEAM_ID="${AGENTPULSE_TEAM_ID:-}"
if [[ -z "$TEAM_ID" && -f macos/signing.local ]]; then
    TEAM_ID="$(sed -n 's/^[[:space:]]*AGENTPULSE_TEAM_ID[[:space:]]*=[[:space:]]*\([A-Z0-9]*\).*/\1/p' macos/signing.local | head -n1)"
fi
if [[ -z "$TEAM_ID" ]]; then
    die "no signing team set.
  Set your Apple Developer team id (10 characters, the OU of your 'Apple Development' certificate) either way:
    export AGENTPULSE_TEAM_ID=XXXXXXXXXX
    echo 'AGENTPULSE_TEAM_ID=XXXXXXXXXX' > macos/signing.local   # untracked
  List candidates with: security find-identity -v -p codesigning"
fi
[[ "$TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || die "AGENTPULSE_TEAM_ID '$TEAM_ID' is not a 10-character team id"

step "Finding a signing identity for team $TEAM_ID"
VALID="$(security find-identity -v -p codesigning | awk '/^ *[0-9]+\)/ {print $2}')"
IDENTITY=""
while read -r hash; do
    [[ -n "$hash" ]] || continue
    ou="$(security find-certificate -a -Z -p 2>/dev/null |
        awk -v h="$hash" '/^SHA-1 hash:/ {take = ($3 == h); next} take {print} take && /END CERT/ {exit}' |
        openssl x509 -noout -subject -nameopt multiline 2>/dev/null |
        sed -n 's/^ *organizationalUnitName *= *//p')" || true # awk exits early: SIGPIPE upstream
    if [[ "$ou" == "$TEAM_ID" ]]; then IDENTITY="$hash"; break; fi
done <<< "$VALID"
[[ -n "$IDENTITY" ]] || die "no valid code-signing identity with OU=$TEAM_ID in the keychain.
  Create an 'Apple Development' certificate for that team in Xcode > Settings > Accounts."
echo "    identity $IDENTITY"

# Wherever cargo really builds, so the bundle never picks up a stale binary.
TARGET="$(cargo metadata --format-version 1 --no-deps | plutil -extract target_directory raw -o - -)" ||
    die "cannot read the target directory from cargo metadata"
[[ -n "$TARGET" ]] || die "cargo metadata reported no target_directory"
OUT="$TARGET/bundle"
APP="$OUT/AgentPulse.app"
XC="$TARGET/xcode"
BIN="$TARGET/release/agentpulse"

VERSION="$(awk '/^\[workspace.package\]/ {p=1; next} /^\[/ {p=0} p && /^version *=/ {gsub(/[" ]/, "", $0); sub(/version=/, ""); print; exit}' Cargo.toml)"
[[ -n "$VERSION" ]] || die "cannot read version from [workspace.package] in Cargo.toml"

# --- build ------------------------------------------------------------------
step "Building the Rust app (release)"
cargo build --release -p agentpulse-app

step "Building the widget extension, bridge and preview tool"
xcodegen generate --quiet --spec macos/project.yml --project macos
xcodebuild -quiet -project macos/AgentPulseWidgets.xcodeproj -alltargets -configuration Release \
    SYMROOT="$XC/build" OBJROOT="$XC/obj" ARCHS="$(uname -m)" \
    MARKETING_VERSION="$VERSION" build
PRODUCTS="$XC/build/Release"

# The gallery placeholder comes from the Rust exporter, so the widget's sleeping
# cat is always the real sprite. Shipped in the extension and beside the preview tool.
"$BIN" --widget-placeholder > "$PRODUCTS/placeholder.json"

# --- assemble ----------------------------------------------------------------
step "Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/PlugIns"
cp "$BIN" "$APP/Contents/MacOS/agentpulse"
cp "$PRODUCTS/libAgentPulseWidgetBridge.dylib" "$APP/Contents/Frameworks/"
cp -R "$PRODUCTS/AgentPulseWidgets.appex" "$APP/Contents/PlugIns/"
mkdir -p "$APP/Contents/PlugIns/AgentPulseWidgets.appex/Contents/Resources"
cp "$PRODUCTS/placeholder.json" "$APP/Contents/PlugIns/AgentPulseWidgets.appex/Contents/Resources/"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key><string>en</string>
	<key>CFBundleExecutable</key><string>agentpulse</string>
	<key>CFBundleIdentifier</key><string>$APP_ID</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key><string>AgentPulse</string>
	<key>CFBundleDisplayName</key><string>AgentPulse</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>CFBundleURLTypes</key>
	<array><dict>
		<key>CFBundleURLName</key><string>$APP_ID</string>
		<key>CFBundleURLSchemes</key><array><string>agentpulse</string></array>
	</dict></array>
</dict>
</plist>
EOF
plutil -lint -s "$APP/Contents/Info.plist"

# --- sign ---------------------------------------------------------------------
# The host app is unsandboxed and needs no entitlements.
WIDGET_ENT="$OUT/widget.entitlements"
cat > "$WIDGET_ENT" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>com.apple.security.app-sandbox</key><true/>
	<key>com.apple.security.temporary-exception.files.home-relative-path.read-only</key>
	<array><string>/Library/Application Support/AgentPulse/Widget/</string></array>
</dict>
</plist>
EOF

sign() {
    if ! with_timeout "$SIGN_TIMEOUT" codesign --force --timestamp=none --sign "$IDENTITY" "$@"; then
        die "codesign failed or timed out after ${SIGN_TIMEOUT}s.
  If macOS was waiting for keychain permission, run scripts/bundle.sh once in a
  terminal you can see and click 'Always Allow' for codesign."
    fi
}
step "Signing (inner code first)"
sign "$APP/Contents/Frameworks/libAgentPulseWidgetBridge.dylib"
sign --entitlements "$WIDGET_ENT" "$APP/Contents/PlugIns/AgentPulseWidgets.appex"
sign "$APP"

step "Verifying"
codesign --verify --deep --strict --verbose=2 "$APP"
echo "--- app entitlements"
codesign -d --entitlements - --xml "$APP" 2>/dev/null | plutil -p - 2>/dev/null || echo "(none)"
echo "--- widget entitlements"
codesign -d --entitlements - --xml "$APP/Contents/PlugIns/AgentPulseWidgets.appex" 2>/dev/null | plutil -p -

echo
echo "Built $APP"
echo "To add the widget: open the app once (open \"$APP\"), then right-click the desktop > Edit Widgets > AgentPulse."
