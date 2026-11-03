#!/usr/bin/env bash
# Run the keychain round-trip test against gnome-keyring in a private D-Bus
# session with a throwaway keyring, so no desktop session or password
# prompt is needed:
#
#   dbus-run-session -- packaging/linux/keyring-test.sh
#
# keyring-test-oo7.sh does the same against oo7-daemon, with locked
# keyrings and their unlock prompts.
set -euo pipefail

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
export XDG_DATA_HOME="$scratch/data"
mkdir -p "$XDG_DATA_HOME"
# --unlock reads the new keyring's password from stdin
eval "$(printf 'corvene' | gnome-keyring-daemon --unlock --components=secrets)"
export GNOME_KEYRING_CONTROL
CORVENE_KEYRING_TEST=1 cargo test -p corvene-platform keychain
# stored again and kept: it must be in the keyring, never in plain text
CORVENE_KEYRING_TEST=1 CORVENE_KEYRING_KEEP=1 cargo test -p corvene-platform keychain
ls "$XDG_DATA_HOME"/keyrings/*.keyring >/dev/null
if grep -rq gho_secret "$XDG_DATA_HOME"; then
    echo "token found in plain text under $XDG_DATA_HOME" >&2
    exit 1
fi
echo "keyring: token stored encrypted"
