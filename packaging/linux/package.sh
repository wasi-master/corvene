#!/bin/bash
# Build Corvene's Linux packages from a release build, for this machine's
# architecture (amd64 / x86_64 or arm64 / aarch64) or, cross-compiled, for
# TARGET (i686-unknown-linux-gnu, armv7-unknown-linux-gnueabihf):
#   target/linux/corvene_<version>_<amd64|arm64|i386|armhf>.deb
#   target/linux/corvene-<version>-1.<x86_64|aarch64|i686|armv7hl>.rpm      (when rpmbuild is found)
#   target/linux/Corvene-<version>-linux-<x86_64|aarch64|i686|armhf>.tar.gz
#   target/linux/Corvene-<version>-<x86_64|aarch64|i686|armhf>.AppImage   (when appimagetool is found)
# and with FULL=1 the "full" variant of each (every tree-sitter grammar
# compiled in): corvene-full_….deb, corvene-full-….rpm, Corvene-Full-….tar.gz
# and Corvene-Full-….AppImage. The full packages replace the default ones
# (same files, same desktop entry), so only one of the two is installed.
#
# Layout (all of them):
#   usr/lib/corvene/corvene          the app
#   usr/lib/corvene/bin/corvene      the command line tool (corvene.sh)
#   usr/bin/corvene -> ../lib/corvene/bin/corvene   (not in the AppImage)
#   usr/share/applications/com.wasimaster.corvene.desktop
#   usr/share/icons/hicolor/{256x256,scalable}/apps/com.wasimaster.corvene.*
#   usr/share/metainfo/com.wasimaster.corvene.metainfo.xml
# The .tar.gz holds that `usr` tree under Corvene[-Full]-<version>-linux-<arch>/:
# the AUR package (packaging/aur), the Flatpak (packaging/flatpak) and the
# snap (packaging/snap) are made from it.
#
# Env: SKIP_BUILD=1 (reuse the release binary), PACKAGE_BIN=<binary> (package
# that binary instead, e.g. CI's debug build), APPIMAGETOOL=<path>,
# FORMATS="deb rpm tar appimage" (a subset), TARGET=<Rust target>
# (cross-compile: needs the target's C compiler and its development
# libraries, the way release.yml installs them; the build machine's
# appimagetool fetches the target's AppImage runtime), FULL=1.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
VERSION="$(cargo metadata --no-deps --format-version 1 |
  python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "corvene"))')"
# TARGET's architecture, else the build machine's (release.yml builds the
# 64-bit ones natively)
case "${TARGET:-$(uname -m)}" in
  x86_64*) ARCH_DEB=amd64 ARCH_RPM=x86_64 ARCH=x86_64 ;;
  aarch64* | arm64) ARCH_DEB=arm64 ARCH_RPM=aarch64 ARCH=aarch64 ;;
  i686*) ARCH_DEB=i386 ARCH_RPM=i686 ARCH=i686 ;;
  armv7*) ARCH_DEB=armhf ARCH_RPM=armv7hl ARCH=armhf ;;
  *) echo "unsupported architecture ${TARGET:-$(uname -m)}" >&2; exit 1 ;;
esac
OUT="$ROOT/target/linux"
ID=com.wasimaster.corvene
FORMATS=" ${FORMATS:-deb rpm tar appimage} "
wants() { [[ "$FORMATS" == *" $1 "* ]]; }

# the package names of the variant, and the other variant's (which it replaces)
if [[ "${FULL:-0}" == "1" ]]; then
  PKG=corvene-full OTHER_PKG=corvene NAME=Corvene-Full FEATURES=(--features full)
else
  PKG=corvene OTHER_PKG=corvene-full NAME=Corvene FEATURES=()
fi

if [ -z "${SKIP_BUILD:-}" ] && [ -z "${PACKAGE_BIN:-}" ]; then
  cargo build --release -p corvene ${TARGET:+--target "$TARGET"} "${FEATURES[@]}"
fi
BIN="${PACKAGE_BIN:-$ROOT/target/${TARGET:+$TARGET/}release/corvene}"
[ -x "$BIN" ] || { echo "no $BIN" >&2; exit 1; }

mkdir -p "$OUT"
WORK="$(mktemp -d "$OUT/stage.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

SUMMARY="Native Git client that looks and works like GitHub Desktop"
DESCRIPTION="Corvene is a native reimplementation of GitHub Desktop 3.6.6: the same
changes, history, branches and pull request workflow, without Electron."

# stage the shared tree into $1
stage() {
  local root="$1"
  install -Dm755 "$BIN" "$root/usr/lib/corvene/corvene"
  install -Dm755 packaging/linux/corvene.sh "$root/usr/lib/corvene/bin/corvene"
  install -Dm644 packaging/linux/$ID.desktop "$root/usr/share/applications/$ID.desktop"
  install -Dm644 packaging/linux/$ID.metainfo.xml "$root/usr/share/metainfo/$ID.metainfo.xml"
  sed -i -e "s|@VERSION@|$VERSION|" -e "s|@DATE@|$(date -u +%Y-%m-%d)|" "$root/usr/share/metainfo/$ID.metainfo.xml"
  install -Dm644 assets/icon/Corvene-256.png "$root/usr/share/icons/hicolor/256x256/apps/$ID.png"
  install -Dm644 assets/icon/Corvene.svg "$root/usr/share/icons/hicolor/scalable/apps/$ID.svg"
  install -Dm644 LICENSE "$root/usr/share/licenses/$PKG/LICENSE"
}

# the system tree (.deb, .rpm, .tar.gz): the app, with `corvene` on PATH
SYSTEM="$WORK/system"
stage "$SYSTEM"
mkdir -p "$SYSTEM/usr/bin"
ln -s ../lib/corvene/bin/corvene "$SYSTEM/usr/bin/corvene"

# ---- .deb -------------------------------------------------------------
if wants deb; then
  DEB="$WORK/deb"
  cp -a "$SYSTEM" "$DEB"
  # Debian keeps licenses in /usr/share/doc/<package>/copyright
  mkdir -p "$DEB/usr/share/doc/$PKG"
  mv "$DEB/usr/share/licenses/$PKG/LICENSE" "$DEB/usr/share/doc/$PKG/copyright"
  rm -rf "$DEB/usr/share/licenses"
  SIZE_KB="$(du -sk "$DEB/usr" | cut -f1)"
  mkdir -p "$DEB/DEBIAN"
  cat > "$DEB/DEBIAN/control" <<EOF
Package: $PKG
Version: $VERSION
Architecture: $ARCH_DEB
Maintainer: Corvene <corvene@wasimaster.com>
Installed-Size: $SIZE_KB
Depends: git (>= 1:2.38), libc6 (>= 2.35), libxcb1, libxkbcommon0, libxkbcommon-x11-0, libfontconfig1, libfreetype6, libvulkan1, libasound2t64 | libasound2, xdg-utils, perl
Recommends: mesa-vulkan-drivers, gnome-keyring | kwalletmanager | keepassxc, hunspell-en-us, fonts-noto-core
Conflicts: $OTHER_PKG
Replaces: $OTHER_PKG
Section: devel
Priority: optional
Homepage: https://github.com/wasi-master/corvene
Description: $SUMMARY
$(printf '%s\n' "$DESCRIPTION" | sed 's/^/ /')
EOF
  cat > "$DEB/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
EOF
  cp "$DEB/DEBIAN/postinst" "$DEB/DEBIAN/postrm"
  chmod 755 "$DEB/DEBIAN/postinst" "$DEB/DEBIAN/postrm"
  dpkg-deb --root-owner-group --build "$DEB" "$OUT/${PKG}_${VERSION}_${ARCH_DEB}.deb" >/dev/null
  echo "built $OUT/${PKG}_${VERSION}_${ARCH_DEB}.deb"
fi

# ---- .rpm -------------------------------------------------------------
# rpmbuild packs the staged tree as it is (no %build): Fedora, openSUSE and
# RHEL-likes take it. Requires are the ELF libraries rpm finds in the binary
# plus the tools Corvene runs. A 32-bit or cross-built package is made with
# --target: no strip or debuginfo pass, which would need the target's tools.
if wants rpm; then
  if ! command -v rpmbuild >/dev/null; then
    echo "rpmbuild not found: skipping the .rpm" >&2
  else
    RPM="$WORK/rpm"
    mkdir -p "$RPM"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
    # rpm versions cannot hold "-": a pre-release (0.2.0-beta.1) sorts
    # before its release with "~"
    RPM_VERSION="${VERSION//-/\~}"
    cat > "$RPM/SPECS/$PKG.spec" <<EOF
Name: $PKG
Version: $RPM_VERSION
Release: 1
Summary: $SUMMARY
License: MIT
URL: https://github.com/wasi-master/corvene
# files rather than package names: Fedora, openSUSE and RHEL name them differently
Requires: /usr/bin/git, /usr/bin/perl, /usr/bin/xdg-open
Recommends: mesa-vulkan-drivers
Conflicts: $OTHER_PKG
AutoReqProv: yes
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none
%global __brp_strip %{nil}

%description
$DESCRIPTION

%install
mkdir -p %{buildroot}
cp -a "$SYSTEM/usr" %{buildroot}/

%post
update-desktop-database -q /usr/share/applications >/dev/null 2>&1 || :
gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor >/dev/null 2>&1 || :

%postun
update-desktop-database -q /usr/share/applications >/dev/null 2>&1 || :
gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor >/dev/null 2>&1 || :

%files
%license /usr/share/licenses/$PKG/LICENSE
/usr/bin/corvene
/usr/lib/corvene
/usr/share/applications/$ID.desktop
/usr/share/icons/hicolor/256x256/apps/$ID.png
/usr/share/icons/hicolor/scalable/apps/$ID.svg
/usr/share/metainfo/$ID.metainfo.xml
EOF
    rpmbuild --quiet -bb --target "$ARCH_RPM-linux" \
      --define "_topdir $RPM" \
      --define "_rpmdir $RPM/RPMS" \
      "$RPM/SPECS/$PKG.spec"
    find "$RPM/RPMS" -name '*.rpm' -exec cp {} "$OUT/" \;
    echo "built $OUT/$PKG-$RPM_VERSION-1.$ARCH_RPM.rpm"
  fi
fi

# ---- .tar.gz ------------------------------------------------------------
if wants tar; then
  TAR_DIR="$NAME-$VERSION-linux-$ARCH"
  mkdir -p "$WORK/tar"
  cp -a "$SYSTEM" "$WORK/tar/$TAR_DIR"
  tar -C "$WORK/tar" --owner=0 --group=0 --numeric-owner -czf "$OUT/$TAR_DIR.tar.gz" "$TAR_DIR"
  echo "built $OUT/$TAR_DIR.tar.gz"
fi

# ---- AppImage -----------------------------------------------------------
if wants appimage; then
  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [ -z "$TOOL" ]; then
    echo "appimagetool not found: skipping the AppImage" >&2
    exit 0
  fi
  APPDIR="$WORK/Corvene.AppDir"
  stage "$APPDIR"
  # AppImage desktop entries name the executable without a path
  sed -e 's|^Exec=.*|Exec=corvene %U|' -e 's|^TryExec=.*||' \
    packaging/linux/$ID.desktop > "$APPDIR/$ID.desktop"
  cp assets/icon/Corvene-256.png "$APPDIR/$ID.png"
  install -Dm755 packaging/linux/AppRun "$APPDIR/AppRun"
  # appimagetool reads the architecture from ARCH
  export ARCH
  "$TOOL" --no-appstream "$APPDIR" "$OUT/$NAME-${VERSION}-${ARCH}.AppImage" >/dev/null
  echo "built $OUT/$NAME-${VERSION}-${ARCH}.AppImage"
fi
