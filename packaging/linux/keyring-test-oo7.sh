#!/usr/bin/env bash
# The keychain tests against oo7-daemon (GNOME OS's secret service,
# desktop#22762) in a private D-Bus session with throwaway keyrings:
#
#   dbus-run-session -- packaging/linux/keyring-test-oo7.sh
#
# OO7_DAEMON names the daemon (default: oo7-daemon on PATH; `cargo install
# oo7-daemon` builds one). Without a graphical session oo7 asks for a
# keyring password through org.freedesktop.secrets.CliPrompter, which
# oo7-prompter.py stands in for (it needs python3-dbus and python3-gi).
#
#  1. the login keyring unlocked at start (`--login`): the token round
#     trips and is only on disk encrypted;
#  2. the login keyring locked, the prompter answers: Corvene unlocks it
#     through the service and stores the token;
#  3. locked, the prompter is dismissed: a clear error, nothing stored;
#  4. locked, no prompter at all (oo7's "User interaction failed" case):
#     a clear error;
#  5. locked, the prompter never answers: the call gives up with a clear
#     error instead of hanging.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
oo7="${OO7_DAEMON:-oo7-daemon}"
python="${PYTHON:-python3}"
password=corvene

scratch="$(mktemp -d)"
daemon=""
prompter=""
cleanup() {
    [[ -n "$prompter" ]] && kill "$prompter" 2>/dev/null || true
    [[ -n "$daemon" ]] && kill "$daemon" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$scratch"
}
trap cleanup EXIT
export XDG_DATA_HOME="$scratch/data"
mkdir -p "$XDG_DATA_HOME"

# the test binary once, so a scenario does not wait for cargo
cargo test -p corvene-platform --no-run 2>/dev/null
test_bin="$(cargo test -p corvene-platform --no-run --message-format=json 2>/dev/null |
    "$python" -c 'import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m["target"]["name"] == "corvene_platform" and m.get("executable"):
        print(m["executable"])')"
[[ -x "$test_bin" ]] || { echo "no corvene-platform test binary" >&2; exit 1; }

start_daemon() {
    # $1: "login" to unlock the login keyring with $password at start
    if [[ "${1:-}" == login ]]; then
        printf '%s' "$password" | "$oo7" --login --replace >>"$scratch/daemon.log" 2>&1 &
    else
        "$oo7" --replace >>"$scratch/daemon.log" 2>&1 </dev/null &
    fi
    daemon=$!
    for _ in $(seq 50); do
        dbus-send --session --print-reply --dest=org.freedesktop.DBus / \
            org.freedesktop.DBus.GetNameOwner string:org.freedesktop.secrets >/dev/null 2>&1 && return
        sleep 0.1
    done
    echo "oo7-daemon did not start" >&2
    cat "$scratch/daemon.log" >&2
    exit 1
}

stop_daemon() {
    kill "$daemon" 2>/dev/null || true
    wait "$daemon" 2>/dev/null || true
    daemon=""
}

start_prompter() {
    "$python" "$here/oo7-prompter.py" "$@" >>"$scratch/prompter.log" 2>&1 &
    prompter=$!
    for _ in $(seq 50); do
        dbus-send --session --print-reply --dest=org.freedesktop.DBus / \
            org.freedesktop.DBus.GetNameOwner string:org.freedesktop.secrets.CliPrompter >/dev/null 2>&1 && return
        sleep 0.1
    done
    echo "the prompter did not start" >&2
    cat "$scratch/prompter.log" >&2
    exit 1
}

stop_prompter() {
    kill "$prompter" 2>/dev/null || true
    wait "$prompter" 2>/dev/null || true
    prompter=""
}

no_plain_text() {
    if grep -rq gho_secret "$XDG_DATA_HOME"; then
        echo "token found in plain text under $XDG_DATA_HOME" >&2
        exit 1
    fi
}

# a scenario that must fail with a message containing $1
expect_error() {
    CORVENE_KEYRING_TEST=1 CORVENE_KEYRING_EXPECT_ERROR="$1" CORVENE_KEYRING_TIMEOUT_SECS="${2:-120}" \
        "$test_bin" keychain::tests::a_keyring_that_cannot_be_used_is_explained --exact --nocapture 2>&1 |
        grep -E '^keyring error:|test result' >&2
    CORVENE_KEYRING_TEST=1 CORVENE_KEYRING_EXPECT_ERROR="$1" CORVENE_KEYRING_TIMEOUT_SECS="${2:-120}" \
        "$test_bin" keychain::tests::a_keyring_that_cannot_be_used_is_explained --exact -q >/dev/null 2>&1
}

round_trip() {
    CORVENE_KEYRING_TEST=1 "$test_bin" keychain::tests::tokens_round_trip_through_the_os_store --exact -q >/dev/null
    CORVENE_KEYRING_TEST=1 CORVENE_KEYRING_KEEP=1 \
        "$test_bin" keychain::tests::tokens_round_trip_through_the_os_store --exact -q >/dev/null
}

echo "1. login keyring unlocked at start"
start_daemon login
round_trip
stop_daemon
ls "$XDG_DATA_HOME"/keyrings/v1/*.keyring >/dev/null
no_plain_text
echo "   ok: stored, and only encrypted on disk"

echo "2. locked, the prompter gives the password"
start_daemon
start_prompter answer "$password"
round_trip
grep -q '^prompted:' "$scratch/prompter.log"
stop_prompter
stop_daemon
no_plain_text
echo "   ok: unlocked through the service"

echo "3. locked, the prompt is dismissed"
start_daemon
start_prompter dismiss
expect_error 'is locked and could not be unlocked'
stop_prompter
stop_daemon
echo "   ok"

echo "4. locked, no prompter"
start_daemon
expect_error 'is locked and could not be unlocked'
stop_daemon
echo "   ok"

echo "5. locked, the prompter never answers"
start_daemon
start_prompter hang
expect_error 'did not answer within 3 seconds' 3
stop_prompter
stop_daemon
echo "   ok"

no_plain_text
echo "keyring (oo7): tokens stored encrypted, locked keyrings unlocked or explained"
