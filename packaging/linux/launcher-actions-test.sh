#!/usr/bin/env bash
# The launcher's recent repositories (flag 434-launcher-recent-repositories,
# desktop#20943) as a desktop sees them: Corvene writes its user entry over
# the installed one, desktop-file-validate accepts it, GIO (what GNOME
# Shell reads entries with) lists the repositories as actions, and
# launching one runs the installed program with the repository's
# x-corvene://openLocalRepo URL. With an empty list the user entry goes.
#
#   packaging/linux/launcher-actions-test.sh
#
# Needs desktop-file-validate (desktop-file-utils), python3-gi and GIO.
set -euo pipefail

python="${PYTHON:-python3}"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
export XDG_DATA_HOME="$scratch/home/.local/share"
export XDG_DATA_DIRS="$scratch/usr/share"
mkdir -p "$XDG_DATA_HOME" "$XDG_DATA_DIRS/applications"

# the installed entry, its program a stub that records what it is given
stub="$scratch/usr/lib/corvene/corvene"
mkdir -p "$(dirname "$stub")"
printf '#!/bin/sh\nprintf "%%s\\n" "$@" >> %s/launched\n' "$scratch" >"$stub"
chmod +x "$stub"
sed -e "s|^Exec=.*|Exec=$stub %U|" -e "s|^TryExec=.*|TryExec=$stub|" \
    packaging/linux/com.wasimaster.corvene.desktop \
    >"$XDG_DATA_DIRS/applications/com.wasimaster.corvene.desktop"
user_entry="$XDG_DATA_HOME/applications/com.wasimaster.corvene.desktop"

run() {
    CORVENE_LAUNCHER_TEST="$1" cargo test -q -p corvene-platform --lib \
        desktop_entry::tests::set_recent_repositories_keeps_the_users_entry -- --exact \
        >/dev/null 2>&1
}

echo "1. two recent repositories"
run write
[[ -f "$user_entry" ]]
desktop-file-validate "$user_entry"
grep -E '^(Actions|Name|Exec)=|^\[' "$user_entry" | sed 's/^/   /'

"$python" - "$user_entry" "$scratch/launched" <<'PY'
import sys, time, os
from gi.repository import Gio
path, launched = sys.argv[1], sys.argv[2]
# the user's entry shadows the installed one under the same id
info = Gio.DesktopAppInfo.new("com.wasimaster.corvene.desktop")
assert info.get_filename() == path, info.get_filename()
actions = info.list_actions()
names = [info.get_action_name(a) for a in actions]
print("   GIO actions:", list(zip(actions, names)))
assert actions == ["repo-1", "repo-2"], actions
assert names == ["corvene", "My notes"], names
info.launch_action("repo-2", None)
for _ in range(50):
    if os.path.exists(launched) and open(launched).read():
        break
    time.sleep(0.1)
got = open(launched).read().split("\n")[0]
print("   launched with:", got)
assert got == "x-corvene://openLocalRepo/home/me/My%20notes", got
PY

echo "2. no recent repositories"
run clear
[[ ! -e "$user_entry" ]]
echo "   ok: the user entry is gone, the installed one shows again"
echo "launcher actions: written, valid, listed and launched by GIO, removed"
