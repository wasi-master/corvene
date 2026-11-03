#!/usr/bin/env bash
# desktop#8783: GitHub Enterprise behind a reverse proxy that asks for
# HTTP Basic credentials of its own (flag 530-front-door-credentials).
# front-door-server.py runs a stub Enterprise server (OAuth token exchange,
# the API, git over smart HTTP) behind such a proxy, once in each of its
# modes:
#
#  - cookie: the proxy takes its credentials in Authorization once and
#    hands out a session cookie (the reporter's proxy);
#  - header: the proxy reads Proxy-Authorization on every request.
#
# For each, the core test signs in (token exchange, then the account),
# calls the API and clones, fetches and pushes with git: refused without
# the proxy's credentials or with wrong ones, through with them, the token
# never replaced.
#
#   packaging/linux/front-door-test.sh
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
python="${PYTHON:-python3}"
scratch="$(mktemp -d)"
server=""
cleanup() {
    [[ -n "$server" ]] && kill "$server" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$scratch"
}
trap cleanup EXIT

export GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=T GIT_AUTHOR_EMAIL=t@example.com \
    GIT_COMMITTER_NAME=T GIT_COMMITTER_EMAIL=t@example.com

# the repository on the server
mkdir -p "$scratch/repos/octocat"
seed="$scratch/seed"
git init -q -b main "$seed"
printf 'hello\n' >"$seed/README.md"
git -C "$seed" add README.md
git -C "$seed" -c commit.gpgsign=false commit -q -m init
git clone -q --bare "$seed" "$scratch/repos/octocat/hello.git"
git -C "$scratch/repos/octocat/hello.git" config http.receivepack true

# git's questions for GitHub's own challenge: the login and the token
cat >"$scratch/askpass.sh" <<'ASKPASS'
#!/bin/sh
case "$1" in
  Username*) echo octocat ;;
  Password*) echo stub-token ;;
esac
ASKPASS
chmod +x "$scratch/askpass.sh"

port="$("$python" -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"

for mode in cookie header; do
    echo "== front door: $mode"
    rm -rf "$scratch/work" "$scratch/refused"
    "$python" "$here/front-door-server.py" "$scratch/repos" "$port" "$mode" 2>"$scratch/$mode.log" &
    server=$!
    for _ in $(seq 50); do
        curl -s -o /dev/null "http://127.0.0.1:$port/" && break
        sleep 0.1
    done
    CORVENE_FRONT_DOOR_TEST="http://127.0.0.1:$port" CORVENE_FRONT_DOOR_SCRATCH="$scratch" \
        cargo test -q -p corvene-core --lib \
        front_door::tests::requests_and_git_get_through_a_front_door -- --exact --nocapture 2>&1 |
        grep -E "test result|panicked|assert|left|right|sign-in|clone|fetch|push" | sed "s/^/   /"
    kill "$server"
    wait "$server" 2>/dev/null || true
    server=""
    echo "   what the front door saw:"
    sed 's/^/     /' "$scratch/$mode.log"
    # pushed through it
    git -C "$scratch/repos/octocat/hello.git" log --format='   server has: %s' -1 main
    git -C "$scratch/repos/octocat/hello.git" log --format=%s -1 main | grep -qx pushed
    git -C "$scratch/repos/octocat/hello.git" reset -q --soft HEAD~1 2>/dev/null ||
        git -C "$scratch/repos/octocat/hello.git" update-ref refs/heads/main main~1
done
echo "front door: sign-in, API, clone, fetch and push got through in both modes"
