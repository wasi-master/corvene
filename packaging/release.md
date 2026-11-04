# Releasing Corvene

How a tagged commit becomes the artefacts the self-updater and the Homebrew
cask consume (Linux: see [Linux](#linux)). Nothing here needs an Apple Developer ID: the
bundle is signed with a self-signed certificate, and the self-updater checks
a download against the sha256 GitHub lists for the asset.

## Code-signing certificate (one-time, maintainer's machine)

macOS lets an app read its Keychain items (the account tokens,
`corvene_platform::keychain`) without asking only while its designated
requirement matches the one that created them. An ad-hoc signature's
requirement is the binary's hash, so every build and every update would ask
for the login password again. Corvene is therefore signed with a
self-signed certificate: its requirement, `identifier
"com.wasimaster.corvene" and certificate leaf = H"…"`, stays the same across
builds. Gatekeeper treats it like an ad-hoc signature (hence the cask's
quarantine-clearing `postflight_steps`).

```bash
packaging/signing-cert.sh create   # → login keychain, ~/.corvene-signing/corvene-signing.{p12,password}
```

- Keep the `.p12` and its password in the password manager and add them as
  the GitHub Actions secrets `MACOS_SIGNING_P12` (base64 of the file) and
  `MACOS_SIGNING_P12_PASSWORD`. On another machine:
  `packaging/signing-cert.sh import corvene-signing.p12`.
- `packaging/bundle.sh` signs with it whenever the keychain has it (the
  first run asks to use the key: Always Allow), else ad-hoc;
  `CORVENE_SIGN_IDENTITY=-` forces ad-hoc. `packaging/release.sh` refuses a
  bundle that is not signed with it (`ALLOW_ADHOC=1` for tests).
- Losing it is not fatal: a new certificate means
  one more password prompt per Keychain item on every install. The first
  self-signed release does the same for installs of ad-hoc builds.
- `codesign -d -r- /Applications/Corvene.app` shows the requirement.

## Release checklist

1. Bump `version` in `Cargo.toml` (`[workspace.package]`), run
   `packaging/acknowledgements.sh` if dependencies changed, commit,
   `git tag v<version>`. Pushing the tag runs steps 2, 3 and 5 in CI
   ([Releasing from CI](#releasing-from-ci)); the steps below are the local
   path.
2. `packaging/release.sh` (see `packaging/release.sh --help`): release build,
   `packaging/bundle.sh release`, the universal binary when both target
   directories exist, `Corvene-<version>-macos-universal.zip` (+ `.dmg`), the
   `Corvene-Full-…` variant when `FULL=1`, and the cask's sha256. Output
   lands in `target/release-assets/`. The `.dmg` comes from
   `packaging/dmg.sh`: the app beside an `/Applications` link over
   `assets/dmg/background.tiff` (regenerate it from `background.svg` with
   `packaging/dmg.sh --background`), laid out by Finder over AppleScript,
   so it needs a logged-in session. The **Disk images** workflow
   (`dmg.yml`, run by hand) rebuilds a published release's `.dmg` assets
   from its own `.zip` assets, screenshots one, and with `replace` swaps
   them in.
3. Create the GitHub release for the tag and upload the `.zip` and `.dmg`
   files in `target/release-assets/`. The self-updater reads
   `GET /repos/wasi-master/corvene/releases/latest`, picks the `-macos-` `.zip`
   of its variant (`Corvene-Full-…` for a Full build) whose name contains
   `universal` (else the machine's architecture)
   and checks the download against the asset's `digest` (the sha256 GitHub
   computes at upload and shows next to the asset). The release body is
   Markdown; list items tagged `[New]` / `[Improved]` / `[Fixed]` /
   `[Added]` / `[Removed]` become the Release Notes dialog's entries
   (`corvene_core::release_notes`).
4. Update `packaging/homebrew/Casks/corvene.rb` with the version, the
   sha256 `release.sh` printed and the two AppImages' sha256
   (`packaging/homebrew/stamp.py`), and push it to the `homebrew-corvene`
   tap (`packaging/homebrew/README.md`).
5. When a pack changed: [Packs](#packs).

## Packs

The on-demand packs (`crates/corvene-packs`: the tree-sitter grammars, one
archive per OS and architecture) are not assets of the app's releases. They
live on one rolling release tagged `packs`, which is created with
`--latest=false` so `releases/latest` (the updater's feed, the README's
download link) never points at it, next to `packs-manifest.json`. The app
reads `…/releases/download/packs/packs-manifest.json`; every entry carries
the archive's URL and sha256, and the download is checked against it.

A pack is installed by version, so a published `(pack, version, platform)`
is never built or uploaded again: bump the pack's version
(`corvene_grammars::PACK_VERSION`) to ship new grammars. The manifest keeps
the older entries (with their `min_app`) for the apps that still need them.

Every platform Corvene ships for has packs: `macos-{aarch64,x86_64}`,
`linux-{x86_64,aarch64,i686,armv7}`, `windows-{x86_64,aarch64,i686}` and
`android-{aarch64,armv7a,x86_64,i686}` (`corvene_packs::pack_target`; the
architecture is spelled like the start of the Rust target). The 32-bit
Linux and Windows ones are cross-built (`PACK_TARGETS=<Rust target>
packaging/packs.sh`: on Linux with the target's gcc cross toolchain, which
clang finds, and `NM=<triplet>-nm`; on Windows with the MSVC libraries of
that architecture), and their jobs may fail without holding up the others.
A target for which no grammar builds gets no pack at all, never an empty
one (a published pack is never rebuilt).

By hand: `packaging/packs.sh` on each platform (`PACK_OS=android` for the
Android ones), add the `packs` entries of its `packs-manifest.json` to the
release's, then upload the archives and the merged manifest:

```bash
gh release create packs --latest=false --title Packs --notes "…"   # once
gh release upload packs --clobber target/release-assets/packs/*.zip packs-manifest.json
```

## Releasing from CI

`.github/workflows/release.yml` runs on a `v*` tag push. It runs no fmt,
clippy or tests (`ci.yml` does on every push to `main`): tag a commit whose
CI run is green. Every platform builds at once and hands its assets on as
workflow artifacts:

- `plan`: the version (fails when the tag is not `v<Cargo.toml version>`)
  and which packs the `packs` release lacks.
- `macos-build`: one `macos-26` runner (Xcode 26; its `actool` crashes on a
  macOS 15 host) per architecture and variant, a `--no-default-features`
  release build each (`--features full` for `Corvene-Full`).
- `macos-assets`: `packaging/release.sh` with `SKIP_BUILD=1` on those four
  binaries (lipo, bundle signed with the certificate from
  `packaging/signing-cert.sh ci`, zip, dmg), per variant once for the
  universal bundle and once per architecture (`ARCH=arm64` / `x86_64`:
  `Corvene-<version>-macos-arm64.zip`, a smaller download). The updater and
  the cask take the universal zip.
- `linux` (x86_64 and aarch64, and cross-compiled i686 and armhf; each in
  both variants): the `.deb`, `.rpm`, `.tar.gz` and AppImage.
- `flatpak` and `snap` (x86_64 and aarch64, both variants), made from the
  `linux` jobs' `.tar.gz`.
- `windows` (x86_64, aarch64, i686; both variants): the installer, the
  `.msi` and the portable `.zip`.
- `android` (default: both flavours, per ABI and universal, plus the Play
  bundle; full: the foss flavour per ABI).
- `packs`: only the platforms `plan` listed; `publish-packs` uploads them to
  the `packs` release and merges their entries into its manifest.
- `publish`: once every build is through, a **draft** release
  with the installers (`.zip`, `.dmg`, `.deb`, `.rpm`, `.tar.gz`,
  `.AppImage`, `.flatpak`, `.snap`, `-setup.exe`, `.msi`, signed `.apk`),
  its notes ending in a Downloads table of them
  (`packaging/release-table.py`: platforms as rows, architectures as
  columns, one short link per asset; the Full builds are left to the
  Assets list so the body stays under the 10,000 characters GitHub's
  /releases page previews),
  the `cask` artifact: `corvene.rb` with the version and every sha256
  filled in (also in the run summary), and the `aur` artifact
  ([AUR](#aur)).

Repository secrets: `MACOS_SIGNING_P12`, `MACOS_SIGNING_P12_PASSWORD`, and
optionally `CORVENE_GITHUB_CLIENT_SECRET` (without it the `307-sign-in-flow`
"auto" default uses the device flow) and the Android keystore's.

After the run: write the release notes in the draft above its `## Downloads`
section (the Release Notes dialog skips that section; a later run rewrites
it and keeps the rest), publish it (the
self-updater ignores drafts), then copy the `cask` artifact's `corvene.rb`
to the tap and the `aur` artifact's two folders to the AUR.
"Run workflow" with `linux_release` set to a published release's tag
(`gh workflow run release.yml --ref main -f linux_release=v0.1.0`) skips
macOS and Android, builds the Linux assets from that branch, replaces them
on the release and stamps the whole cask (the macOS hash from the release's
zip). `Cargo.toml` must still carry that version. The tag stays where it is,
so the Linux binaries are newer than the tagged source.

"Run workflow" by hand builds the same assets as workflow artifacts without
a release, and on `main` it is also what saves the dependency caches the tag
runs read (a tag's run can read `main`'s caches but not another tag's) (`full` input: skip the Full variant; `publish_packs`: rebuild
every pack and publish it to the `packs` release anyway).

## How the updater uses the assets

`corvene_platform::updater` (checked at launch with a 15–60 s jitter and
every four hours, release builds only):

1. `GET …/releases/latest` (no token; `CORVENE_UPDATE_FEED` overrides the
   URL for testing). A tag newer than the running version by semver wins;
   drafts are skipped.
2. `<zip>` is downloaded to `~/Library/Caches/Corvene/updates/` and its
   sha256 compared with the asset's `digest` from the feed. A release whose
   asset lists no sha256 digest is refused.
3. The "Corvene N is available" banner appears. "Install and Restart" (banner,
   Release Notes dialog, About) unpacks the zip with `ditto` (keeps the
   signature), renames the running bundle to `Corvene.app.old`, moves
   the new bundle in, schedules `open -n` for after this process exits and
   quits. The next launch deletes `Corvene.app.old`.
4. A bundle under `/opt/homebrew/Caskroom`, `/usr/local/Caskroom`, or in
   `/Applications` while a `corvene` cask is installed, is never swapped: the
   banner says `brew upgrade corvene`.

Files the app downloads itself carry no quarantine attribute, so the self-signed
update launches without Gatekeeper's "Open Anyway" dance.

## Linux

`packaging/linux/package.sh` builds two assets from a release build into
`target/linux/`:

- `corvene_<version>_amd64.deb` / `corvene_<version>_arm64.deb`
- `Corvene-<version>-x86_64.AppImage` / `Corvene-<version>-aarch64.AppImage`

- `corvene-<version>-1.x86_64.rpm` / `corvene-<version>-1.aarch64.rpm`
  (when `rpmbuild` is installed: `apt install rpm` on Debian and Ubuntu)
- `Corvene-<version>-linux-x86_64.tar.gz` / `…-linux-aarch64.tar.gz`: the
  `.deb`'s `usr` tree in a `Corvene-<version>-linux-<arch>/` folder, to unpack
  into `/` or `/usr/local` (`tar -xzf … --strip-components=1 -C /usr/local`
  puts `bin/corvene` on the `PATH`; the desktop entry starts
  `/usr/lib/corvene/corvene`, so only an unpacked-into-`/` tree has a working
  entry) or to run in place (`usr/lib/corvene/corvene`). The AUR packages,
  the Flatpak and the snap are made from it.

for the architecture of the machine it runs on (release.yml builds both,
the arm64 ones on an `ubuntu-24.04-arm` runner). `FORMATS="deb rpm tar
appimage"` picks some of them.

`FULL=1` builds the `Corvene-Full` variant of each (every tree-sitter
grammar compiled in): `corvene-full_….deb`, `corvene-full-….rpm`,
`Corvene-Full-<version>-linux-<arch>.tar.gz` and
`Corvene-Full-<version>-<arch>.AppImage`. The `corvene-full` packages
conflict with and replace `corvene` (the same files), so a machine has one
or the other. A Full AppImage updates itself to the next Full AppImage,
like every Full build (the updater only picks assets of its own variant).

The `.rpm` is the staged tree as it is (no `%build`); its `Requires` are the
ELF libraries rpm finds plus `/usr/bin/git`, `/usr/bin/perl` and
`/usr/bin/xdg-open` (files, since Fedora, openSUSE and RHEL name the
packages differently). Cross-built 32-bit packages are `rpmbuild --target`
with the strip and debuginfo passes off (`i686`, `armv7hl`). Both the
`.deb` and the `.rpm` install `usr/share/metainfo/com.wasimaster.corvene.metainfo.xml`
(AppStream, for software centres).

`TARGET=i686-unknown-linux-gnu` or `TARGET=armv7-unknown-linux-gnueabihf`
cross-compiles the 32-bit ones (`_i386.deb` / `-i686.AppImage`,
`_armhf.deb` / `-armhf.AppImage`): release.yml does on the runner of the
same family, with the target's gcc and the `:i386` / `:armhf` development
libraries from Debian multiarch (the linkers are in `.cargo/config.toml`).
Those jobs may fail without holding up a release: nothing upstream is
tested on 32-bit Linux. Their tree-sitter packs are cross-built
([Packs](#packs)); there is no Homebrew cask, Flatpak or snap for them.

A `v<version>` tag does this in release.yml's `linux` jobs; `publish` adds
both to the draft release. The machine's tree-sitter packs
(`tree-sitter-{all,rest}-<v>-linux-<arch>.zip`, `.so` units; `packs.sh`
builds shared objects on Linux) come from the `packs` jobs
([Packs](#packs)).

Only the AppImage updates itself. The updater picks
`Corvene-*-<arch>.AppImage` (the machine's `x86_64` / `aarch64` / `i686` /
`armhf`) from the
latest release, downloads it to `$XDG_CACHE_HOME/corvene/updates/`
(`~/.cache/corvene/updates/`) and checks its sha256. "Install and Restart"
copies the image next to the running one (`$APPIMAGE`, which must be a file
in a folder the user can write), checks that copy again, makes it executable, fsyncs it, renames it over
`$APPIMAGE` and starts it once the old process has exited.

The AppImage also installs its own desktop entry
(`corvene_platform::desktop_entry`, run from `url_schemes::register` at
every launch with `$APPIMAGE` set): the packaged
`com.wasimaster.corvene.desktop` with `Exec=<$APPIMAGE> %U` and
`TryExec=<$APPIMAGE>` goes to `$XDG_DATA_HOME/applications/`, the two icons
from the mounted image (`$APPDIR/usr/share/icons/hicolor`) to
`$XDG_DATA_HOME/icons/hicolor/{256x256,scalable}/apps/`, and `xdg-mime` then
makes it the `x-corvene` / `x-corvene-auth` handler. The entry carries
`X-Corvene-Installer=appimage`: only an entry with that line is rewritten
(when the image moves or the template changes), a user entry without it and
the `.deb`'s entry under `$XDG_DATA_DIRS` are left alone, and with the
`.deb`'s entry present nothing is written. A launch that is not from an
AppImage removes the entry once its image is gone. `package.sh` must keep
shipping the icons at those paths in the AppDir.

A `.deb`, `.rpm`, `.tar.gz`, AUR, Flatpak or snap install is never updated
in place (none of them is an AppImage): like a
Homebrew cask on macOS, the banner and About only say that the release is
available and to update with the package manager.

The Homebrew cask installs the same AppImage as
`~/Applications/Corvene.AppImage`. An image in `~/Applications` while a
`corvene` cask exists (`Caskroom/corvene` under `$HOMEBREW_PREFIX`,
`/home/linuxbrew/.linuxbrew` or `~/.linuxbrew`) is the cask's and is never
swapped: the banner says `brew upgrade corvene`. The
`publish` job stamps both AppImages' sha256 into the cask.

### Flatpak

`packaging/flatpak/build.sh <Corvene[-Full]-<version>-linux-<arch>.tar.gz> [out]`
builds `Corvene[-Full]-<version>-<arch>.flatpak`, a single-file bundle of
`com.wasimaster.corvene` (`packaging/flatpak/com.wasimaster.corvene.yml`,
runtime `org.freedesktop.Platform` 26.08 from Flathub, which the bundle
names so `flatpak install` fetches it). x86_64 and aarch64 only, on a
machine of that architecture, with `flatpak` and `flatpak-builder`
installed. The sandbox builds its own git (the runtime has none; no Perl,
Python or Tcl/Tk parts, no git-lfs) and may read and write the home folder,
talk to the Secret Service (account tokens), notifications and the SSH
agent. Not on Flathub: install the bundle with
`flatpak install --user Corvene-<version>-x86_64.flatpak` (the other
variant: `--reinstall`, same application id). Limits of the sandbox:
repositories outside the home folder need
`flatpak override --user --filesystem=<path> com.wasimaster.corvene`, and
External Editor / Open in Shell only find what is inside the sandbox.
release.yml's `flatpak` jobs build them; a failure there does not hold up
the release.

### Snap

`packaging/snap/prepare.sh <archive> <dir>` writes `<dir>/snap/snapcraft.yaml`
(the version filled in) and the archive's `usr` tree; `snapcraft pack` in
`<dir>` builds the snap (core24, strict confinement, the `gnome` extension
for the desktop libraries and GPU drivers; git, git-lfs and ssh from Ubuntu
24.04 inside it, `GIT_EXEC_PATH` pointing there). release.yml's `snap` jobs
build it with `snapcore/action-build` (LXD) on x86_64 and arm64 runners as
`Corvene[-Full]-<version>-<arch>.snap`; a failure there does not hold up the
release. Not in the Snap Store: `sudo snap install --dangerous
Corvene-<version>-x86_64.snap`, then `sudo snap connect
corvene:password-manager-service` (account tokens) and `sudo snap connect
corvene:ssh-keys` (git over SSH); neither connects on its own.

### AUR

`packaging/aur/stamp.py --version <v> --assets <folder> [--out target/aur]`
writes `corvene-bin/` and `corvene-full-bin/` (each a `PKGBUILD` and its
`.SRCINFO`) from the `.tar.gz` files in `<folder>`: binary packages that
unpack the release's archive into `/` for x86_64, aarch64, i686 and armv7h
(an architecture without an archive is left out). They conflict with each
other. The `publish` job leaves them as the `aur` artifact; after
publishing the release, copy each folder into a clone of
`ssh://aur@aur.archlinux.org/<name>.git` and push (the maintainer's AUR SSH
key; nothing in CI holds one).

## Android

`packaging/android/build.sh release` builds the `foss` and `play` packages
with every ABI (arm64-v8a, armeabi-v7a, x86_64, x86) in each; `PER_ABI=1`
also one per ABI. release.yml uploads all ten as
`Corvene-<version>-android-<foss|play>-<arm64|armv7|x86_64|x86|universal>.apk`; the
`play` bundle (`.aab`) stays a workflow artifact for Google Play.
`FULL=1` compiles every tree-sitter grammar into the library: release.yml's
full job builds the foss flavour so, one package per ABI
(`Corvene-Full-<version>-android-foss-<abi>.apk`; one with all four would be
several hundred MB). It has the same application id and key, so it
installs over the default foss package and back. The play flavour has no
full variant: Google Play delivers the grammars as an on-demand module.

They are signed with the key from `packaging/android/keystore.sh create`
(`~/.corvene-signing/corvene-android.jks` and `.password`; needs a JDK's
`keytool`, so set `JAVA_HOME`). `keystore.sh secrets` stores it as the
`CORVENE_ANDROID_KEYSTORE_BASE64`, `CORVENE_ANDROID_KEYSTORE_PASSWORD`,
`CORVENE_ANDROID_KEY_ALIAS` and `CORVENE_ANDROID_KEY_PASSWORD` repository
secrets; `eval "$(packaging/android/keystore.sh env)"` signs a local
release build. Without them the packages are unsigned and stay workflow
artifacts. Android updates a package only with one signed by the same key:
keep the keystore and its password in the password manager, a lost key
means every user uninstalls first.

## Windows

`packaging/windows/package.ps1` builds the Windows packages from a release
build into `target\windows\`:

- `Corvene-<version>-<x86_64|aarch64|i686>-setup.exe`: the per-user
  installer, which updates itself
- `Corvene-<version>-<arch>.msi`: the machine-wide package (WiX Toolset 5,
  `packaging/windows/corvene.wxs`; skipped when `wix` is not on the `PATH`:
  `dotnet tool install --global wix --version 5.0.2`; WiX 6 and later need
  the Open Source Maintenance Fee EULA)
- `Corvene-<version>-windows-<arch>-portable.zip`: a `Corvene` folder with
  `corvene.exe` and `bin\corvene.bat`, no installer
- the same as `Corvene-Full-<version>-…` with `FULL=1` (every tree-sitter
  grammar compiled in)

for the architecture of the machine it runs on, or for `TARGET=<Rust
target>`. It needs Inno Setup 6.7 or 7 (`iscc.exe`; `ISCC` names it when it
is not in its default folder; CI installs the pinned 7.1.0 with
`.github/actions/inno-setup`). The installer
(`packaging/windows/corvene.iss`) is per user and asks for no
administrator rights: `%LOCALAPPDATA%\Programs\Corvene\corvene.exe`, the
command line tool `bin\corvene.bat` with its folder on the user's `PATH`
and in `App Paths` (so `corvene` works from Win+R), a Start menu shortcut
(whose AppUserModelID notifications are shown under, and whose toast
activator CLSID names `corvene.exe` as the COM server that gets their
clicks, `corvene_platform::notifications`), the `x-corvene` and
`x-corvene-auth` URL schemes and an uninstaller. The wizard follows the
system's light or dark mode and language (Inno Setup's translations; the
texts Corvene adds are English), needs Windows 10 1809, and offers two
tasks: "Open in Corvene" in Explorer's folder menus (off by default) and,
only when no `git.exe` is on the `PATH` or in Git for Windows' folders, a
MinGit for Corvene (`packaging/windows/mingit.iss`: the release, size and
sha256 of each architecture's zip, written by `packaging/windows/mingit.py`
from Git for Windows' latest release; run it to move to a newer git).
Setup downloads that zip from GitHub, checks it and unpacks it into
`<app>\git`, where `corvene_git::detect` looks after the `PATH`.

A running Corvene is closed by the Restart Manager before its files are
replaced and started again afterwards (`RegisterApplicationRestart`,
`corvene_platform::windows`); the uninstaller asks to close it (the
`CorveneRunning` mutex) and leaves the settings in `%APPDATA%\Corvene`,
removing only the app, its MinGit and `%LOCALAPPDATA%\Corvene\Cache`.

A `v<version>` tag does this in release.yml's `windows` jobs, one per
architecture and variant (x86_64 and aarch64 natively, i686 cross-compiled
on the x86_64 runner), and `publish` adds the installers to the draft
release with the other platforms' assets. A failure there does not hold up
the other platforms.

The installers are Authenticode-signed only when there is a certificate:
`SIGN_PFX=<file>` and `SIGN_PFX_PASSWORD` make `package.ps1` sign
`corvene.exe` with `signtool` and hand the same command to Inno Setup as its
Sign Tool for the installer and the uninstaller; in release.yml they come
from the `WINDOWS_SIGNING_PFX_BASE64` (the `.pfx`, base64) and
`WINDOWS_SIGNING_PFX_PASSWORD` repository secrets. Corvene has no
certificate yet (one is bought from a certificate authority), so SmartScreen
warns about the download and a PC with Smart App Control on refuses it.

The `packs` job builds the Windows tree-sitter packs
(`tree-sitter-{all,rest}-<v>-windows-<x86_64|aarch64|i686>.zip`, DLL units) on
Windows runners (i686 cross-built on the x86_64 one): `packaging/packs.sh` in Git Bash, with LLVM's clang. By hand that
needs Python as `python` (or `PYTHON=<exe>`), clang on the `PATH` and the
tree-sitter CLI (`CORVENE_TREE_SITTER`).

An installed Corvene updates itself. The updater picks
`Corvene-*-<arch>-setup.exe` from the latest release, downloads it to
`%LOCALAPPDATA%\Corvene\Cache\updates\` and checks it against the sha256
GitHub lists for the asset. "Install and Restart" copies the installer,
checks the copy, quits,
and a hidden PowerShell runs the copy with `/VERYSILENT` once Corvene has
exited and then starts the new Corvene. A Corvene that was not installed by
the installer (`cargo run`, the portable zip, the `.msi`: no `unins000.exe`
next to it) only says that the release is available. A Full install updates
to the release's `Corvene-Full-…-setup.exe`.

The `.msi` is for deployment to many machines (`msiexec /i
Corvene-<version>-x86_64.msi /qn`, Group Policy, Intune), like GitHub
Desktop's: per machine into `Program Files\Corvene` (`Program Files (x86)`
for i686), `bin` on the system `PATH`, an all-users Start menu shortcut with
the AppUserModelID. A newer `.msi` replaces it (MajorUpgrade; the default
and Full packages share the upgrade code, so either replaces the other). It
writes no URL schemes: Corvene registers `x-corvene` / `x-corvene-auth` for
the user who starts it (`url_schemes::register`). Its version is the
release's without a pre-release suffix (Windows Installer versions are
numeric).

`packaging/windows/corvene.ico` is generated from the app icon by
`packaging/windows/make-ico.ps1`; run it again when the icon changes.

## Testing the flow locally

```bash
cargo build -p corvene && packaging/bundle.sh
# a "new" version: copy the bundle, bump CFBundleShortVersionString, re-sign, zip
cp -R target/bundle/Corvene.app /tmp/new/ && /usr/libexec/PlistBuddy -c 'Set :CFBundleShortVersionString 9.9.9' /tmp/new/Corvene.app/Contents/Info.plist
codesign --force --sign "${CORVENE_SIGN_IDENTITY:--}" /tmp/new/Corvene.app
(cd /tmp/new && ditto -c -k --sequesterRsrc --keepParent Corvene.app Corvene-9.9.9-macos-universal.zip)
shasum -a 256 /tmp/new/Corvene-9.9.9-macos-universal.zip
# a fake feed: latest.json with tag_name "v9.9.9" and one asset: its
# browser_download_url at http://127.0.0.1:8765/ and "digest": "sha256:<that hash>"
(cd /tmp/new && python3 -m http.server 8765)
CORVENE_UPDATE_CHECK=1 CORVENE_UPDATE_FEED=http://127.0.0.1:8765/latest.json open -n target/bundle/Corvene.app
```

`CORVENE_POPUP=update-available[:brew]` shows the banner, About and Release
Notes with a sample update without any feed.
