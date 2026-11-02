#!/usr/bin/env bash
# Build the widget extension and the WidgetCenter library into an app bundle
# (flag 433-macos-widgets), for each architecture of the app's binary.
#
#   packaging/widgets/build.sh <Corvene.app> <signing identity> <version> <build>
#
# No Xcode project: swiftc from the Command Line Tools is enough. The extension
# needs the app sandbox entitlement or pluginkit silently drops it.
set -euo pipefail

APP="$1"; IDENTITY="$2"; VERSION="$3"; BUILD="$4"
HERE="$(cd "$(dirname "$0")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
SDK="$(xcrun --sdk macosx --show-sdk-path)"
APPEX="$APP/Contents/PlugIns/CorveneWidgets.appex"
FRAMEWORKS="$APP/Contents/Frameworks"
mkdir -p "$APPEX/Contents/MacOS" "$FRAMEWORKS"

widgets=(); shims=()
for arch in $(lipo -archs "$APP/Contents/MacOS/corvene"); do
  swiftc -O -parse-as-library -application-extension -sdk "$SDK" \
    -target "$arch-apple-macos14.0" \
    -Xlinker -e -Xlinker _NSExtensionMain \
    -o "$TMP/widgets-$arch" \
    "$HERE/WidgetViews.swift" "$HERE/CorveneWidgets.swift"
  swiftc -O -parse-as-library -emit-library -sdk "$SDK" \
    -target "$arch-apple-macos14.0" \
    -Xlinker -install_name -Xlinker @rpath/libCorveneWidgetCenter.dylib \
    -o "$TMP/shim-$arch.dylib" \
    "$HERE/WidgetCenterShim.swift"
  widgets+=("$TMP/widgets-$arch"); shims+=("$TMP/shim-$arch.dylib")
done
lipo -create "${widgets[@]}" -output "$APPEX/Contents/MacOS/CorveneWidgets"
lipo -create "${shims[@]}" -output "$FRAMEWORKS/libCorveneWidgetCenter.dylib"

sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" \
  "$HERE/Info.plist" > "$APPEX/Contents/Info.plist"

codesign --force --sign "$IDENTITY" --timestamp=none "$FRAMEWORKS/libCorveneWidgetCenter.dylib" >/dev/null
codesign --force --sign "$IDENTITY" --timestamp=none \
  --entitlements "$HERE/widgets.entitlements" "$APPEX" >/dev/null
