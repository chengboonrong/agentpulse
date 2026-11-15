#!/usr/bin/env bash
# Builds <cargo target dir>/bundle/AgentPulse.app (target/ unless CARGO_TARGET_DIR
# or build.target-dir says otherwise) with its WidgetKit extension and signs it
# with the hardened runtime.
#
#   scripts/bundle.sh             Apple Development certificate, for this Mac.
#   scripts/bundle.sh --dmg       The same, plus AgentPulse-<version>.dmg. Not
#                                 notarized: another Mac blocks it until the user
#                                 allows it in System Settings > Privacy & Security.
#   scripts/bundle.sh --release   Developer ID Application certificate, secure
#                                 timestamp, and a notarized, stapled
#                                 AgentPulse-<version>.dmg that opens on any Mac.
#
# Team id: AGENTPULSE_TEAM_ID, else AGENTPULSE_TEAM_ID=... in macos/signing.local
# (untracked). The signing identity is the valid code-signing certificate of the
# right kind whose OU is that team; it is selected by hash, never by name.
# --release also needs notarytool credentials, either of:
#   - an App Store Connect API key: AGENTPULSE_NOTARY_KEY_ID and AGENTPULSE_NOTARY_ISSUER
#     (env or signing.local), with the key at AGENTPULSE_NOTARY_KEY (env), default
#     ~/.appstoreconnect/private_keys/AuthKey_<key id>.p8. No keychain item needed.
#   - otherwise, a keychain profile named by AGENTPULSE_NOTARY_PROFILE (env or signing.local),
#     default "agentpulse-notary":
#       xcrun notarytool store-credentials agentpulse-notary --apple-id <id> --team-id <team>
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

RELEASE=0
DMG=0
case "${1:-}" in
    "") ;;
    --dmg) DMG=1 ;;
    --release) RELEASE=1 DMG=1 ;;
    *) echo "usage: scripts/bundle.sh [--dmg | --release]" >&2; exit 2 ;;
esac

die() { echo "error: $*" >&2; exit 1; }
step() { echo "==> $*"; }
# macOS has no timeout(1); perl's alarm is enough to stop a codesign stuck on a keychain dialog.
with_timeout() { perl -e 'alarm shift; exec @ARGV or die "exec $ARGV[0]: $!"' "$@"; }

for tool in cargo xcodebuild xcodegen xcrun codesign security openssl plutil ditto hdiutil shasum; do
    command -v "$tool" >/dev/null || die "$tool not found (Rust: rustup.rs; xcodegen: brew install xcodegen; the rest ship with Xcode)"
done

# --- team id and identity -------------------------------------------------
# Reads KEY=value from the untracked macos/signing.local.
local_setting() {
    [[ -f macos/signing.local ]] || return 0
    sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\([A-Za-z0-9._-]*\).*/\1/p" macos/signing.local | head -n1
}
TEAM_ID="${AGENTPULSE_TEAM_ID:-$(local_setting AGENTPULSE_TEAM_ID)}"
if [[ -z "$TEAM_ID" ]]; then
    die "no signing team set.
  Set your Apple Developer team id (10 characters, the OU of your 'Apple Development' certificate) either way:
    export AGENTPULSE_TEAM_ID=XXXXXXXXXX
    echo 'AGENTPULSE_TEAM_ID=XXXXXXXXXX' > macos/signing.local   # untracked
  List candidates with: security find-identity -v -p codesigning"
fi
[[ "$TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || die "AGENTPULSE_TEAM_ID '$TEAM_ID' is not a 10-character team id"

if [[ $RELEASE == 1 ]]; then KIND="Developer ID Application"; else KIND="Apple Development"; fi
NOTARY_PROFILE="${AGENTPULSE_NOTARY_PROFILE:-$(local_setting AGENTPULSE_NOTARY_PROFILE)}"
NOTARY_PROFILE="${NOTARY_PROFILE:-agentpulse-notary}"
NOTARY_KEY_ID="${AGENTPULSE_NOTARY_KEY_ID:-$(local_setting AGENTPULSE_NOTARY_KEY_ID)}"
NOTARY_ISSUER="${AGENTPULSE_NOTARY_ISSUER:-$(local_setting AGENTPULSE_NOTARY_ISSUER)}"
if [[ -n "$NOTARY_KEY_ID" ]]; then
    NOTARY_KEY="${AGENTPULSE_NOTARY_KEY:-$HOME/.appstoreconnect/private_keys/AuthKey_$NOTARY_KEY_ID.p8}"
    NOTARY_AUTH=(--key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER")
else
    NOTARY_AUTH=(--keychain-profile "$NOTARY_PROFILE")
fi
if [[ $RELEASE == 1 ]]; then
    for tool in xcrun spctl; do command -v "$tool" >/dev/null || die "$tool not found"; done
    if [[ -n "$NOTARY_KEY_ID" ]]; then
        [[ -n "$NOTARY_ISSUER" ]] || die "AGENTPULSE_NOTARY_KEY_ID is set but AGENTPULSE_NOTARY_ISSUER is not"
        [[ -f "$NOTARY_KEY" ]] || die "no API key file at $NOTARY_KEY (set AGENTPULSE_NOTARY_KEY)"
        step "Notarizing with API key $NOTARY_KEY_ID (it takes precedence over a keychain profile)"
    else
        step "Notarizing with keychain profile '$NOTARY_PROFILE'"
    fi
    # Fail before the long build, not after it.
    xcrun notarytool history "${NOTARY_AUTH[@]}" >/dev/null 2>&1 ||
        die "notarytool cannot sign in with ${NOTARY_AUTH[*]:0:2}. Either set
  AGENTPULSE_NOTARY_KEY_ID and AGENTPULSE_NOTARY_ISSUER for an App Store Connect API key,
  or create a keychain profile once with:
    xcrun notarytool store-credentials $NOTARY_PROFILE --apple-id <your Apple ID> --team-id $TEAM_ID"
fi

step "Finding a '$KIND' identity for team $TEAM_ID"
VALID="$(security find-identity -v -p codesigning | awk '/^ *[0-9]+\)/ {print $2}')"
IDENTITY=""
while read -r hash; do
    [[ -n "$hash" ]] || continue
    subject="$(security find-certificate -a -Z -p 2>/dev/null |
        awk -v h="$hash" '/^SHA-1 hash:/ {take = ($3 == h); next} take {print} take && /END CERT/ {exit}' |
        openssl x509 -noout -subject -nameopt multiline 2>/dev/null)" || true # awk exits early: SIGPIPE upstream
    ou="$(sed -n 's/^ *organizationalUnitName *= *//p' <<< "$subject")"
    cn="$(sed -n 's/^ *commonName *= *//p' <<< "$subject")"
    if [[ "$ou" == "$TEAM_ID" && "$cn" == "$KIND:"* ]]; then IDENTITY="$hash"; break; fi
done <<< "$VALID"
[[ -n "$IDENTITY" ]] || die "no valid '$KIND' certificate with OU=$TEAM_ID in the keychain.
  Create one for that team in Xcode > Settings > Accounts > Manage Certificates
  (Developer ID needs a paid Apple Developer Program membership and the Account Holder role)."
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

step "Building the widget extension, bridges and preview tool"
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
cp "$PRODUCTS/libAgentPulseLinkBridge.dylib" "$APP/Contents/Frameworks/"
cp -R "$PRODUCTS/AgentPulseWidgets.appex" "$APP/Contents/PlugIns/"
mkdir -p "$APP/Contents/PlugIns/AgentPulseWidgets.appex/Contents/Resources"
cp "$PRODUCTS/placeholder.json" "$APP/Contents/PlugIns/AgentPulseWidgets.appex/Contents/Resources/"
# App icon from the Icon Composer document: Assets.car carries the macOS 26 light, dark
# and tinted renditions, AppIcon.icns the icon for macOS 14 and 15.
mkdir -p "$APP/Contents/Resources"
xcrun actool macos/AppIcon.icon --compile "$APP/Contents/Resources" --app-icon AppIcon \
    --platform macosx --minimum-deployment-target 14.0 \
    --output-partial-info-plist "$OUT/icon-info.plist" >/dev/null
[[ -f "$APP/Contents/Resources/AppIcon.icns" && -f "$APP/Contents/Resources/Assets.car" ]] ||
    die "actool did not produce AppIcon.icns and Assets.car (needs Xcode 26 or later)"
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
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundleIconName</key><string>AppIcon</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>NSLocalNetworkUsageDescription</key><string>AgentPulse connects directly to your paired iPhone on this network.</string>
	<key>NSBonjourServices</key><array><string>_agentpulse._tcp</string></array>
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

# Hardened runtime in both modes, so a development build runs under the same
# rules a notarized one does. Only a release gets a secure timestamp (it asks
# Apple's timestamp server).
if [[ $RELEASE == 1 ]]; then TIMESTAMP=--timestamp; else TIMESTAMP=--timestamp=none; fi
sign() {
    if ! with_timeout "$SIGN_TIMEOUT" codesign --force --options runtime "$TIMESTAMP" --sign "$IDENTITY" "$@"; then
        die "codesign failed or timed out after ${SIGN_TIMEOUT}s.
  If macOS was waiting for keychain permission, run scripts/bundle.sh once in a
  terminal you can see and click 'Always Allow' for codesign."
    fi
}
step "Signing (inner code first)"
sign "$APP/Contents/Frameworks/libAgentPulseWidgetBridge.dylib"
sign "$APP/Contents/Frameworks/libAgentPulseLinkBridge.dylib"
sign --entitlements "$WIDGET_ENT" "$APP/Contents/PlugIns/AgentPulseWidgets.appex"
sign "$APP"

step "Verifying"
codesign --verify --deep --strict --verbose=2 "$APP"
echo "--- app entitlements"
codesign -d --entitlements - --xml "$APP" 2>/dev/null | plutil -p - 2>/dev/null || echo "(none)"
echo "--- widget entitlements"
codesign -d --entitlements - --xml "$APP/Contents/PlugIns/AgentPulseWidgets.appex" 2>/dev/null | plutil -p -

if [[ $DMG == 1 ]]; then
    IMAGE="$OUT/AgentPulse-$VERSION.dmg"
    step "Making $IMAGE"
    STAGE="$OUT/dmg"
    rm -rf "$STAGE" "$IMAGE"
    mkdir -p "$STAGE"
    ditto "$APP" "$STAGE/AgentPulse.app"
    ln -s /Applications "$STAGE/Applications"
    # The mounted volume shows the app icon. The custom-icon flag must be set on the volume
    # root itself (hdiutil drops it from the source folder), so go through a writable image.
    cp "$APP/Contents/Resources/AppIcon.icns" "$STAGE/.VolumeIcon.icns"
    RW="$OUT/dmg-rw.dmg"
    rm -f "$RW"
    hdiutil create -quiet -volname "AgentPulse $VERSION" -srcfolder "$STAGE" -fs HFS+ -format UDRW "$RW"
    MNT="$OUT/dmg-mnt"
    mkdir -p "$MNT"
    trap 'hdiutil detach -quiet -force "$MNT" 2>/dev/null || true' EXIT
    hdiutil attach -quiet -nobrowse -mountpoint "$MNT" "$RW"
    xcrun SetFile -a C "$MNT"
    hdiutil detach -quiet "$MNT" || hdiutil detach -quiet -force "$MNT"
    trap - EXIT
    hdiutil convert -quiet "$RW" -format UDZO -o "$IMAGE"
    rm -rf "$STAGE" "$RW" "$MNT"
    sign "$IMAGE"
    if [[ $RELEASE == 1 ]]; then
        step "Notarizing (uploads the signed image to Apple; usually a few minutes)"
        xcrun notarytool submit "$IMAGE" "${NOTARY_AUTH[@]}" --wait
        xcrun stapler staple "$IMAGE"
        xcrun stapler validate "$IMAGE"
        step "Gatekeeper assessment"
        spctl --assess --type open --context context:primary-signature --verbose=2 "$IMAGE"
    fi
    shasum -a 256 "$IMAGE"
fi

echo
echo "Built $APP"
echo "To add the widget: open the app once (open \"$APP\"), then right-click the desktop > Edit Widgets > AgentPulse."
