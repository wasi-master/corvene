#!/usr/bin/env bash
# Assemble Corvene.app from a cargo build. Signed with the self-signed
# "Corvene Self-Signed" certificate when the keychain has it
# (packaging/signing-cert.sh), else ad-hoc; never with a Developer ID.
#
#   packaging/bundle.sh            # debug build  -> target/bundle/Corvene.app
#   packaging/bundle.sh release    # release build -> target/bundle/Corvene.app
#   packaging/bundle.sh release-fast  # any other profile's binary likewise
#   OPEN=1 packaging/bundle.sh     # also launch it
#   CORVENE_SIGN_IDENTITY=- …      # ad-hoc even when the certificate is there
set -euo pipefail

PROFILE="${1:-debug}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/$PROFILE/corvene"
OUT="$ROOT/target/bundle"
APP="$OUT/Corvene.app"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
BUILD="$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || echo 0)"

if [[ ! -x "$BIN" ]]; then
  echo "binary not found: $BIN (run cargo build --profile ${PROFILE/#debug/dev} first)" >&2
  exit 1
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" \
  "$ROOT/packaging/Info.plist" > "$APP/Contents/Info.plist"
cp "$BIN" "$APP/Contents/MacOS/corvene"

# macOS 26 Liquid Glass icon: compile the Icon Composer package (assets/icon/Corvene.icon)
# into Assets.car. Needs actool from Xcode 26+; the Command Line Tools alone do not ship it.
# Without it the bundle falls back to the legacy .icns below (Finder ignores
# CFBundleIconName when Assets.car is absent, so the key is only added on success).
if ACTOOL="$(xcrun --find actool 2>/dev/null)"; then
  "$ACTOOL" "$ROOT/assets/icon/Corvene.icon" \
    --compile "$APP/Contents/Resources" \
    --platform macosx --minimum-deployment-target 26.0 \
    --app-icon Corvene \
    --output-format human-readable-text --errors --warnings \
    --output-partial-info-plist "$OUT/Corvene-icon.plist" >/dev/null
  if [[ ! -f "$APP/Contents/Resources/Assets.car" ]]; then
    echo "actool did not produce Assets.car" >&2
    exit 1
  fi
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string Corvene" "$APP/Contents/Info.plist"
else
  echo "actool not found (Xcode 26+); skipping Liquid Glass icon, using Corvene.icns only" >&2
fi
# Hand-tuned legacy icon (16/32 px variants) wins over the flattened one actool emits.
if [[ -f "$ROOT/assets/Corvene.icns" ]]; then
  cp "$ROOT/assets/Corvene.icns" "$APP/Contents/Resources/Corvene.icns"
fi
printf 'APPL????' > "$APP/Contents/PkgInfo"
# Command line tool (Install Command Line Tool… symlinks it into /usr/local/bin)
install -m 755 "$ROOT/packaging/corvene.sh" "$APP/Contents/Resources/corvene"

# A certificate keeps the designated requirement (identifier + certificate)
# the same from build to build, so Keychain items and privacy permissions
# granted to one build carry over; an ad-hoc one is the binary's hash and
# changes with every build. Either way Gatekeeper still quarantines downloads.
IDENTITY="${CORVENE_SIGN_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
  IDENTITY="-"
  if security find-identity -p codesigning 2>/dev/null | grep -q '"Corvene Self-Signed"'; then
    IDENTITY="Corvene Self-Signed"
  fi
fi
# Widgets (flag `433-macos-widgets`), signed before the app that holds them
"$ROOT/packaging/widgets/build.sh" "$APP" "$IDENTITY" "$VERSION" "$BUILD"
codesign --force --sign "$IDENTITY" --timestamp=none "$APP" >/dev/null

echo "built $APP (v$VERSION build $BUILD, signed: ${IDENTITY/#-/ad-hoc})"
if [[ "${OPEN:-0}" == "1" ]]; then
  open -n "$APP"
fi
