#!/usr/bin/env python3
"""A GitHub Enterprise stand-in behind a reverse proxy that wants HTTP Basic
credentials of its own (desktop#8783), for packaging/linux/front-door-test.sh.

  front-door-server.py <repos> <front port> <mode>

The upstream (a port of its own on 127.0.0.1) answers like GitHub: every
response carries X-GitHub-Request-Id, the OAuth token exchange hands out
"stub-token", /api/v3/user and /api/v3/user/emails want that token, and git's
smart HTTP (git http-backend over <repos>) wants Basic octocat:stub-token.

The front door on <front port> lets a request through when it carries the
proxy's credentials (door:s3cret) the way <mode> says, and answers anything
else with 401 and a Basic challenge of its own:

  cookie   Basic in Authorization, once: the proxy keeps that header for
           itself and sets a session cookie; later requests get through with
           the cookie and their own Authorization (the reporter's proxy)
  header   Basic in Proxy-Authorization, on every request

Every request the front door sees is logged to stderr.
"""

import base64
import http.client
import http.server
import json
import os
import secrets
import subprocess
import sys
import threading

DOOR = "Basic " + base64.b64encode(b"door:s3cret").decode()
TOKEN = "stub-token"
GIT_BASIC = "Basic " + base64.b64encode(f"octocat:{TOKEN}".encode()).decode()
SESSIONS = set()


def log(*parts):
    print(*parts, file=sys.stderr, flush=True)


def describe(value):
    """What an Authorization-like header holds, for the log."""
    if not value:
        return "-"
    if value == DOOR:
        return "door"
    if value in (f"Bearer {TOKEN}", f"token {TOKEN}", GIT_BASIC):
        return "token"
    return "other"


class Upstream(http.server.BaseHTTPRequestHandler):
    repos = ""

    def log_message(self, *args):
        pass

    def reply(self, status, body, headers=()):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("X-GitHub-Request-Id", secrets.token_hex(4))
        for name, value in headers:
            self.send_header(name, value)
        if not any(name.lower() == "content-type" for name, _ in headers):
            self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def body(self):
        length = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(length) if length else b""

    def token_ok(self):
        auth = self.headers.get("Authorization", "")
        return auth in (f"Bearer {TOKEN}", f"token {TOKEN}")

    def handle_any(self):
        path, _, query = self.path.partition("?")
        if path == "/login/oauth/access_token" and self.command == "POST":
            self.body()
            return self.reply(200, {"access_token": TOKEN, "scope": "repo,user,workflow"})
        if path.startswith("/api/v3/"):
            if not self.token_ok():
                return self.reply(401, {"message": "Requires authentication"})
            if path == "/api/v3/user":
                return self.reply(
                    200,
                    {"id": 1, "login": "octocat", "name": "Mona", "avatar_url": None},
                    [("X-OAuth-Scopes", "repo, user, workflow")],
                )
            if path == "/api/v3/user/emails":
                return self.reply(
                    200,
                    [{"email": "mona@example.com", "primary": True, "verified": True}],
                )
            return self.reply(404, {"message": "Not Found"})
        if ".git/" in path:
            if self.headers.get("Authorization") != GIT_BASIC:
                return self.reply(
                    401,
                    b"Authentication required\n",
                    [("WWW-Authenticate", 'Basic realm="GitHub"'), ("Content-Type", "text/plain")],
                )
            return self.git(path, query)
        return self.reply(404, {"message": "Not Found"})

    def git(self, path, query):
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "GIT_PROJECT_ROOT": self.repos,
            "GIT_HTTP_EXPORT_ALL": "1",
            "PATH_INFO": path,
            "QUERY_STRING": query,
            "REQUEST_METHOD": self.command,
            "CONTENT_TYPE": self.headers.get("Content-Type", ""),
            "REMOTE_USER": "octocat",
            "REMOTE_ADDR": "127.0.0.1",
            "GIT_CONFIG_NOSYSTEM": "1",
        }
        body = self.body()
        env["CONTENT_LENGTH"] = str(len(body))
        if self.headers.get("Content-Encoding"):
            env["HTTP_CONTENT_ENCODING"] = self.headers["Content-Encoding"]
        if self.headers.get("Git-Protocol"):
            env["GIT_PROTOCOL"] = self.headers["Git-Protocol"]
        out = subprocess.run(
            ["git", "http-backend"], input=body, env=env, capture_output=True, check=False
        ).stdout
        head, _, data = out.partition(b"\r\n\r\n")
        status, headers = 200, []
        for line in head.decode("latin-1").split("\r\n"):
            name, _, value = line.partition(":")
            if name.lower() == "status":
                status = int(value.strip().split()[0])
            elif name:
                headers.append((name, value.strip()))
        self.reply(status, data, headers)

    do_GET = do_POST = handle_any


class FrontDoor(http.server.BaseHTTPRequestHandler):
    mode = "cookie"
    upstream_port = 0

    def log_message(self, *args):
        pass

    def challenge(self):
        data = b"<html><body>401 Authorization Required</body></html>"
        self.send_response(401)
        self.send_header("WWW-Authenticate", 'Basic realm="Corporate front door"')
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def session(self):
        for part in self.headers.get("Cookie", "").split(";"):
            name, _, value = part.strip().partition("=")
            if name == "frontdoor" and value in SESSIONS:
                return True
        return False

    def handle_any(self):
        # header names in any case, as clients send them
        headers = {k.lower(): v for k, v in self.headers.items()}
        auth = headers.get("authorization")
        proxy_auth = headers.pop("proxy-authorization", None)
        set_cookie = None
        if self.mode == "header":
            allowed = proxy_auth == DOOR
        elif auth == DOOR:
            # the proxy's own credentials: kept by the proxy, a session given
            headers.pop("authorization")
            set_cookie = secrets.token_hex(8)
            SESSIONS.add(set_cookie)
            allowed = True
        else:
            allowed = self.session()
        log(
            f"front door: {self.command} {self.path} "
            f"authorization={describe(auth)} proxy-authorization={describe(proxy_auth)} "
            f"cookie={'yes' if self.session() else '-'} -> {'pass' if allowed else '401'}"
        )
        if not allowed:
            self.challenge()
            return
        length = int(headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else None
        for hop in ("connection", "keep-alive", "host"):
            headers.pop(hop, None)
        conn = http.client.HTTPConnection("127.0.0.1", self.upstream_port, timeout=60)
        conn.request(self.command, self.path, body=body, headers=headers)
        response = conn.getresponse()
        data = response.read()
        self.send_response(response.status)
        for name, value in response.getheaders():
            if name.lower() not in ("connection", "transfer-encoding", "content-length"):
                self.send_header(name, value)
        if set_cookie:
            self.send_header("Set-Cookie", f"frontdoor={set_cookie}; Path=/; HttpOnly")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = handle_any


def main():
    repos, port, mode = sys.argv[1], int(sys.argv[2]), sys.argv[3]
    Upstream.repos = repos
    upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    FrontDoor.mode = mode
    FrontDoor.upstream_port = upstream.server_address[1]
    front = http.server.ThreadingHTTPServer(("127.0.0.1", port), FrontDoor)
    log(f"front door ({mode}) on {port}, upstream on {FrontDoor.upstream_port}")
    front.serve_forever()


if __name__ == "__main__":
    main()
