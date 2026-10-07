"""A stub GitHub REST API for the parity harness (`github_stub: true`).

Corvene's Issues and Releases views (flags `345-issues`, `346-releases`)
read and write through the real API client, which no scenario can point at
GitHub (no account, no network). `start()` serves fixed answers on a free
port, as a GitHub Enterprise Server would under `/api/v3`; the `github:
stub` step then runs the `fake-github` control hook, which signs Corvene in
to `http://127.0.0.1:<port>/api/v3` with an injected token and makes the
fixture repository the stub's `octocat/parity-fixture`.

Dates are fixed far in the past, so relative times stay the same from run
to run ("last year"). A created issue is `#101`; a created release gets
id 9. `generate-notes` answers for any tag; with `GITHUB_STUB_NO_NOTES=1`
it answers 404 (a GitHub Enterprise Server before 3.5), so Corvene builds
the notes from the local history instead.

With a fixture repository (`start(repo)`, the `repo-pull-request` setup) the
stub also serves pull request #7, `feature/login` into `main`, whose head is
the branch's first commit (the second counts as not pushed yet), its check
runs, and the GraphQL operations of `corvene_github::review` by operation
name: the review threads (one open, one resolved range, one outdated, one
pending), the overview, and the mutations, which change the stub's threads
so a reload shows the reply, the resolved state, the new comment or the
submitted review (`348-pull-request-review`).

Several accounts (`527-multiple-accounts`, `github: stub-accounts`): the
token names its account (`stub-token-<login>`; plain `stub-token` is
octocat), `/user` answers with that login, `permissions` (login → admin,
write or read) sets what each sees of the repository, and `requests` logs
(login, method, path) so a driver can check which account a call used.

The Actions view (`351-actions`) reads two workflows (CI, and Release with
`workflow_dispatch` inputs), five runs (one in progress; run 7 is the
failed CI run whose jobs the check runs come from), their jobs, the
branches, tags and environments, and the Release workflow's file. Re-run,
cancel and delete-logs answer as GitHub does and change the stub's runs;
a dispatch adds a queued run dated now. Runs and jobs carry an `ETag` and
answer 304 to a matching `If-None-Match`.

Pull request events (`354-pull-request-event-notifications`): `pr_events`
serves pull requests of two other repositories (`octo-org/website`,
`octo-org/api`) behind `/notifications` (`Last-Modified` / `If-Modified-Since`
304s, `X-Poll-Interval`) and `/search/issues`. Nothing has happened until a
driver calls `stub.pr_events.happen()`, which dates every event now: a review
asked of the account, an approval and a merge by hubot of the account's pull
request, a mention of the account and one of its team, a merge the account did
itself (no notification) and an approval in the fixture repository. `pr_events.refuse = True` answers 403
on `/notifications`, so Corvene falls back to the search.

Issue detail (`345-issues`): #42's description is GitHub-flavoured
Markdown with a table, task list, alert, `<details>`, inline HTML and an
image the stub serves itself (`/assets/screenshot.png`; `{STUB_BASE}` in a
body is the stub's address). The issues carry reactions; #42 has two
comments and #41 thirty-five (pages of `per_page`, a `Link` to the next).
Commenting, closing / reopening (`state_reason`), setting labels and
assignees and locking change the stub's issues, so a reload shows them.

Repository rules (`1225-branch-rules-link`): creating a `release/*` branch
is restricted by a rule nobody can bypass (ruleset 41); a `hotfix/*` name
must end in a number by a ruleset the user always bypasses (42).
"""

from __future__ import annotations

import hashlib
import json
import os
import time
import re
import urllib.parse
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

OWNER, NAME = "octocat", "parity-fixture"
# `1223-checkout-from-fork`: the forks `fixture.FORKS` builds
FORK_OWNERS = ("alice", "bob")


def _user(login: str) -> dict:
    return {"login": login, "id": hash(login) & 0xFFFF, "avatar_url": None, "html_url": f"https://github.com/{login}"}


LABELS = [
    {"name": "bug", "color": "d73a4a", "description": "Something isn't working"},
    {"name": "documentation", "color": "0075ca", "description": "Improvements or additions to documentation"},
    {"name": "enhancement", "color": "a2eeef", "description": "New feature or request"},
    {"name": "good first issue", "color": "7057ff", "description": "Good for newcomers"},
]
ASSIGNEES = [_user("octocat"), _user("mona"), _user("hubot")]


def _reactions(**counts: int) -> dict:
    keys = ["+1", "-1", "laugh", "hooray", "confused", "heart", "rocket", "eyes"]
    named = {"+1": counts.get("plus1", 0), "-1": counts.get("minus1", 0)}
    out = {k: named.get(k, counts.get(k, 0)) for k in keys}
    out["total_count"] = sum(out.values())
    return out


ISSUE_42_BODY = """Corvene crashes right after launch when the repository list is empty.

### Steps

1. Remove every repository
2. Quit and open the app again
3. Watch it ~~start~~ crash

- [x] Reproduced on macOS 15
- [ ] Reproduced on Linux

| Version | OS | Result |
|:--|:-:|--:|
| 0.1.0 | macOS 15.1 | **crash** |
| 0.0.9 | macOS 14.6 | works |
| 0.0.8 | Ubuntu 24.04 | works |

> [!NOTE]
> Only on a fresh install, see `~/Library/Application Support/Corvene`.

<details>
<summary>Crash log</summary>

```
thread 'main' panicked at crates/corvene/src/main.rs:42:5:
index out of bounds: the len is 0 but the index is 0
```

</details>

<p align="center">
  <img src="{STUB_BASE}/assets/screenshot.png" alt="The crash dialog" width="320">
</p>

Press <kbd>Cmd</kbd>+<kbd>Q</kbd> to quit.<br>See #41 and the [docs](https://docs.github.com).<!-- triage: p1 -->"""


def _issue(number: int, title: str, state: str, created: str, labels: list[str], assignees: list[str],
           comments: int = 0, body: str | None = None) -> dict:
    by_name = {l["name"]: l for l in LABELS}
    return {
        "number": number, "id": 1000 + number, "node_id": f"I_stub{number}", "title": title, "state": state,
        "state_reason": "completed" if state == "closed" else None,
        "body": body if body is not None else f"### Steps\n\n1. Open the app\n2. Watch it\n\nSee #{max(number - 1, 1)} for the history.",
        "html_url": f"http://127.0.0.1/{OWNER}/{NAME}/issues/{number}",
        "created_at": created, "updated_at": created, "closed_at": created if state == "closed" else None,
        "user": _user("octocat"), "labels": [by_name[l] for l in labels], "assignees": [_user(a) for a in assignees],
        "comments": comments,
        "reactions": _reactions(plus1=3, eyes=1) if number == 42 else _reactions(),
        "locked": False, "active_lock_reason": None,
    }


def _comment(cid: int, number: int, login: str, created: str, body: str, **reactions: int) -> dict:
    return {"id": cid, "node_id": f"IC_stub{cid}", "body": body, "user": _user(login),
            "html_url": f"http://127.0.0.1/{OWNER}/{NAME}/issues/{number}#issuecomment-{cid}",
            "created_at": created, "updated_at": created, "author_association": "MEMBER",
            "reactions": _reactions(**reactions)}


def _comments() -> dict[int, list[dict]]:
    """Each issue's comments, oldest first."""
    return {
        42: [
            _comment(4201, 42, "mona", "2024-03-02T09:00:00Z",
                     "Same here on **0.1.0**. The log ends with:\n\n```\nindex out of bounds\n```", plus1=2),
            _comment(4202, 42, "hubot", "2024-03-03T12:30:00Z",
                     "> Same here on 0.1.0\n\nFixed on `main`, see the [release notes](https://docs.github.com).",
                     heart=1, rocket=2),
        ],
        41: [_comment(4100 + n, 41, "mona" if n % 2 else "hubot", f"2024-02-{21 + n // 12:02}T{n % 12 + 8:02}:00:00Z",
                      f"Comment number {n + 1}.") for n in range(35)],
        37: [_comment(3700 + n, 37, "octocat", "2024-01-06T10:00:00Z", f"Fixed in v0.0.{n + 7}.")
             for n in range(3)],
    }


def _png(width: int, height: int) -> bytes:
    """A window-ish picture: a title bar and a red alert on a grey page."""
    import struct
    import zlib
    rows = []
    for y in range(height):
        row = bytearray([0])
        for x in range(width):
            if y < 28:
                px = (60, 64, 72)
            elif abs(x - width // 2) < 90 and abs(y - height // 2 - 10) < 50:
                px = (207, 34, 46) if abs(y - height // 2 + 30) < 10 else (246, 248, 250)
            else:
                px = (36 + x * 40 // width, 41 + y * 40 // height, 54)
            row.extend(px)
        rows.append(bytes(row))
    raw = zlib.compress(b"".join(rows))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", raw) + chunk(b"IEND", b"")


SCREENSHOT_PNG = _png(640, 360)


OPEN_ISSUES = [
    _issue(42, "Crash on launch with an empty repository list", "open", "2024-03-01T10:00:00Z", ["bug"], ["octocat"], 2,
           ISSUE_42_BODY),
    _issue(41, "Dark mode for the tutorial panel", "open", "2024-02-20T10:00:00Z", ["enhancement", "good first issue"], [], 35),
    _issue(38, "Document the release process", "open", "2024-02-02T10:00:00Z", ["documentation"], ["mona"], 0),
]
CLOSED_ISSUES = [
    _issue(37, "Typo in the welcome screen", "closed", "2024-01-05T10:00:00Z", ["bug"], ["octocat"], 3),
]


def _release(rid: int, tag: str, name: str, draft: bool, prerelease: bool, created: str, assets: bool = True) -> dict:
    web = f"http://127.0.0.1/{OWNER}/{NAME}"
    return {
        "id": rid, "tag_name": tag, "name": name, "draft": draft, "prerelease": prerelease,
        "body": (f"## What's Changed\n* Keep scroll position on refresh by @octocat in #{rid * 3}\n"
                 f"* Show commit signature status by @mona in #{rid * 3 + 1}\n\n**Full Changelog**: {web}/compare/v0.0.9...{tag}"),
        "html_url": f"{web}/releases/tag/{tag}", "author": _user("octocat"),
        "created_at": created, "published_at": None if draft else created, "target_commitish": "main",
        "tarball_url": f"{web}/archive/refs/tags/{tag}.tar.gz",
        "assets": [] if not assets else [
            {"name": f"Corvene-{tag.lstrip('v')}.dmg", "size": 24117248, "download_count": 128,
             "browser_download_url": f"{web}/releases/download/{tag}/Corvene.dmg"},
            {"name": "SHA256SUMS", "size": 412, "download_count": 9,
             "browser_download_url": f"{web}/releases/download/{tag}/SHA256SUMS"},
        ],
    }


RELEASES = [
    _release(3, "v0.2.0", "", True, False, "2024-04-01T10:00:00Z", assets=False),
    _release(2, "v0.1.1-beta.1", "Beta 1", False, True, "2024-03-10T10:00:00Z"),
    _release(1, "v0.1.0", "First release", False, False, "2024-02-15T10:00:00Z"),
]


PR_NUMBER = 7
PR_NODE = "PR_stub7"
WEB = f"http://127.0.0.1/{OWNER}/{NAME}"


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True).stdout.strip()


class PullRequestFixture:
    """Pull request #7 over the fixture repository, with its review threads
    and the mutations' effects."""

    def __init__(self, repo: Path):
        self.head = _git(repo, "rev-parse", "feature/login~1")
        self.tip = _git(repo, "rev-parse", "feature/login")
        self.base = _git(repo, "rev-parse", "origin/main")
        # the pull request's repository is the fixture's bare origin, as `git
        # remote -v` prints it, so the checked-out branch matches it
        self.clone_url = _git(repo, "remote", "get-url", "origin")
        self.comments = 0
        self.pending_review = "PRR_pending"
        self.submitted = []
        self.conversation = []
        self.threads = [
            self._thread("PRRT_1", "src/main.rs", 4, None, "RIGHT", False, False, [
                self._comment("octocat", "Should `--login` also accept a user name?", 40),
                self._comment("mona", "Not yet: the greeting only changes its words.", 38),
            ]),
            self._thread("PRRT_2", "src/main.rs", 9, 7, "RIGHT", True, False, [
                self._comment("hubot", "Collapse this into one `format!` with a conditional verb.", 50),
            ], resolved_by="mona"),
            self._thread("PRRT_3", "src/main.rs", None, None, "RIGHT", False, True, [
                self._comment("octocat", "Upper-casing the whole greeting also shouts the name.", 60),
            ], original_line=13),
            self._thread("PRRT_4", "README.md", 8, None, "RIGHT", False, False, [
                self._comment("octocat", "Say what a returning user is.", 1, pending=True),
            ]),
        ]

    def _comment(self, login: str, body: str, hours_ago: int, pending: bool = False) -> dict:
        self.comments += 1
        hour = 23 - min(hours_ago, 23)
        return {
            "id": f"PRRC_{self.comments}", "databaseId": 500 + self.comments, "body": body,
            "createdAt": f"2024-03-{1 + hours_ago // 24:02d}T{hour:02d}:00:00Z",
            "updatedAt": f"2024-03-{1 + hours_ago // 24:02d}T{hour:02d}:00:00Z",
            "url": f"{WEB}/pull/{PR_NUMBER}#discussion_r{500 + self.comments}",
            "state": "PENDING" if pending else "SUBMITTED", "outdated": False,
            "viewerCanUpdate": login == "octocat", "viewerCanDelete": login == "octocat",
            "viewerDidAuthor": login == "octocat", "diffHunk": "@@ -1,6 +1,8 @@\n fn main() {",
            "author": {"login": login, "avatarUrl": None}, "originalCommit": {"oid": self.head},
            "commit": {"oid": self.head}, "replyTo": None,
            "pullRequestReview": {"id": self.pending_review if pending else "PRR_1",
                                  "state": "PENDING" if pending else "COMMENTED"},
        }

    def _thread(self, tid: str, path: str, line, start_line, side: str, resolved: bool, outdated: bool,
                comments: list, resolved_by: str | None = None, original_line: int | None = None) -> dict:
        return {
            "id": tid, "path": path, "line": line, "startLine": start_line,
            "originalLine": original_line if original_line is not None else line,
            "originalStartLine": start_line, "diffSide": side, "startDiffSide": side if start_line else None,
            "isResolved": resolved, "isOutdated": outdated,
            "resolvedBy": {"login": resolved_by} if resolved_by else None,
            "viewerCanResolve": True, "viewerCanUnresolve": True, "viewerCanReply": True,
            "comments": {"nodes": comments},
        }

    def pending_count(self) -> int:
        return sum(1 for t in self.threads for c in t["comments"]["nodes"] if c["state"] == "PENDING")

    def rest(self) -> dict:
        repo = {"name": NAME, "owner": _user(OWNER), "html_url": WEB, "clone_url": self.clone_url,
                "default_branch": "main", "private": False, "fork": False, "parent": None,
                "node_id": "R_stub", "permissions": {"admin": True, "push": True, "pull": True}}
        return {
            "number": PR_NUMBER, "title": "Add a --login flag", "state": "open", "draft": False,
            "created_at": "2024-02-28T10:00:00Z", "updated_at": "2024-03-03T10:00:00Z",
            "user": _user("mona"), "body": "Greets returning users with `--login`.\n\nCloses #41.",
            "head": {"ref": "feature/login", "sha": self.head, "repo": repo},
            "base": {"ref": "main", "sha": self.base, "repo": repo},
            "assignees": [], "requested_reviewers": [_user("hubot")],
            "html_url": f"{WEB}/pull/{PR_NUMBER}",
        }

    def overview(self) -> dict:
        pending = [{"id": self.pending_review, "body": "", "author": {"login": "octocat"},
                    "comments": {"totalCount": self.pending_count()}}] if self.pending_count() else []
        reviews = [{"id": "PRR_1", "state": "CHANGES_REQUESTED", "body": "A couple of things before this lands.",
                    "submittedAt": "2024-03-01T12:00:00Z", "url": f"{WEB}/pull/{PR_NUMBER}#pullrequestreview-1",
                    "author": {"login": "hubot", "avatarUrl": None}, "comments": {"totalCount": 1}}]
        reviews += self.submitted
        timeline = [
            {"__typename": "PullRequestCommit", "commit": {"oid": self.head, "messageHeadline": "Add a --login flag",
             "committedDate": "2024-02-28T09:00:00Z", "author": {"name": "Mona", "user": {"login": "mona"}}}},
            {"__typename": "ReviewRequestedEvent", "createdAt": "2024-02-28T10:05:00Z", "actor": {"login": "mona"},
             "requestedReviewer": {"__typename": "User", "login": "hubot"}},
            {"__typename": "IssueComment", "id": "IC_1", "body": "CI is green on this one.",
             "createdAt": "2024-02-29T08:00:00Z", "url": f"{WEB}/pull/{PR_NUMBER}#issuecomment-1",
             "author": {"login": "octocat", "avatarUrl": None}},
            {"__typename": "LabeledEvent", "createdAt": "2024-02-29T09:00:00Z", "actor": {"login": "mona"},
             "label": {"name": "enhancement", "color": "a2eeef"}},
            dict(reviews[0], __typename="PullRequestReview"),
        ] + self.conversation + [dict(r, __typename="PullRequestReview") for r in self.submitted]
        return {
            "id": PR_NODE, "number": PR_NUMBER, "title": "Add a --login flag",
            "body": "Greets returning users with `--login`.\n\n- [x] flag\n- [ ] docs\n\nCloses #41.",
            "state": "OPEN", "isDraft": False, "createdAt": "2024-02-28T10:00:00Z", "mergedAt": None,
            "closedAt": None, "url": f"{WEB}/pull/{PR_NUMBER}", "author": {"login": "mona", "avatarUrl": None},
            "baseRefName": "main", "headRefName": "feature/login", "baseRefOid": self.base,
            "headRefOid": self.head, "isCrossRepository": False, "headRepository": {"nameWithOwner": f"{OWNER}/{NAME}"},
            "mergeable": "MERGEABLE", "reviewDecision": "CHANGES_REQUESTED", "additions": 14, "deletions": 3,
            "changedFiles": 2, "viewerCanUpdate": True,
            "commits": {"totalCount": 1, "nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS"}}}]},
            "labels": {"nodes": [{"name": "enhancement", "color": "a2eeef"}]},
            "milestone": {"title": "v0.2"}, "assignees": {"nodes": [{"login": "mona"}]},
            "reviewRequests": {"nodes": [{"requestedReviewer": {"__typename": "User", "login": "hubot", "avatarUrl": None}}]},
            "latestOpinionatedReviews": {"nodes": reviews},
            "pendingReviews": {"nodes": pending},
            "timelineItems": {"totalCount": len(timeline), "nodes": timeline},
        }

    def find_thread(self, tid: str) -> dict | None:
        return next((t for t in self.threads if t["id"] == tid), None)

    def graphql(self, query: str, variables: dict) -> dict:
        """The answer's `data` for one of `corvene_github::review`'s operations."""
        m = re.match(r"\s*(query|mutation)\s+(\w+)", query)
        op = m.group(2) if m else ""
        v = variables or {}
        if op == "ReviewThreads":
            return {"repository": {"pullRequest": {"id": PR_NODE, "reviewThreads": {
                "pageInfo": {"hasNextPage": False, "endCursor": None}, "nodes": self.threads}}}}
        if op == "PullRequestOverview":
            return {"viewer": {"login": "octocat"}, "repository": {"pullRequest": self.overview()}}
        if op == "AddThreadReply":
            thread = self.find_thread(v.get("thread", ""))
            if thread is None:
                return {"addPullRequestReviewThreadReply": None}
            comment = self._comment("octocat", v.get("body", ""), 0, pending=bool(v.get("review")))
            comment["replyTo"] = {"id": thread["comments"]["nodes"][0]["id"]}
            thread["comments"]["nodes"].append(comment)
            return {"addPullRequestReviewThreadReply": {"comment": {"id": comment["id"]}}}
        if op in ("ResolveThread", "UnresolveThread"):
            thread = self.find_thread(v.get("thread", ""))
            if thread is None:
                return {"resolveReviewThread": None, "unresolveReviewThread": None}
            thread["isResolved"] = op == "ResolveThread"
            thread["resolvedBy"] = {"login": "octocat"} if thread["isResolved"] else None
            key = "resolveReviewThread" if op == "ResolveThread" else "unresolveReviewThread"
            return {key: {"thread": {"id": thread["id"], "isResolved": thread["isResolved"]}}}
        if op == "StartReview":
            return {"addPullRequestReview": {"pullRequestReview": {"id": self.pending_review, "state": "PENDING"}}}
        if op in ("AddReviewThread", "AddSingleComment"):
            inp = v.get("input") if op == "AddReviewThread" else (v.get("threads") or [{}])[0]
            pending = op == "AddReviewThread"
            tid = f"PRRT_{len(self.threads) + 1}"
            thread = self._thread(tid, inp.get("path", ""), inp.get("line"), inp.get("startLine"),
                                  inp.get("side", "RIGHT"), False, False,
                                  [self._comment("octocat", inp.get("body", ""), 0, pending=pending)])
            self.threads.append(thread)
            if pending:
                return {"addPullRequestReviewThread": {"thread": {"id": tid}}}
            return {"addPullRequestReview": {"pullRequestReview": {"id": f"PRR_{tid}", "state": "COMMENTED"}}}
        if op in ("SubmitReview", "AddReview"):
            event = v.get("event", "COMMENT")
            state = {"APPROVE": "APPROVED", "REQUEST_CHANGES": "CHANGES_REQUESTED"}.get(event, "COMMENTED")
            count = 0
            for t in self.threads:
                for c in t["comments"]["nodes"]:
                    if c["state"] == "PENDING":
                        c["state"] = "SUBMITTED"
                        c["pullRequestReview"] = {"id": "PRR_submitted", "state": state}
                        count += 1
            self.submitted.append({"id": "PRR_submitted", "state": state, "body": v.get("body", ""),
                                   "submittedAt": "2024-03-04T10:00:00Z",
                                   "url": f"{WEB}/pull/{PR_NUMBER}#pullrequestreview-9",
                                   "author": {"login": "octocat", "avatarUrl": None},
                                   "comments": {"totalCount": count}})
            key = "submitPullRequestReview" if op == "SubmitReview" else "addPullRequestReview"
            return {key: {"pullRequestReview": {"id": "PRR_submitted", "state": state}}}
        if op == "DeleteReview":
            for t in self.threads:
                t["comments"]["nodes"] = [c for c in t["comments"]["nodes"] if c["state"] != "PENDING"]
            self.threads = [t for t in self.threads if t["comments"]["nodes"]]
            return {"deletePullRequestReview": {"pullRequestReview": {"id": self.pending_review, "state": "DISMISSED"}}}
        if op == "UpdateReviewComment":
            for t in self.threads:
                for c in t["comments"]["nodes"]:
                    if c["id"] == v.get("comment"):
                        c["body"] = v.get("body", "")
            return {"updatePullRequestReviewComment": {"pullRequestReviewComment": {"id": v.get("comment")}}}
        if op == "DeleteReviewComment":
            for t in self.threads:
                t["comments"]["nodes"] = [c for c in t["comments"]["nodes"] if c["id"] != v.get("comment")]
            self.threads = [t for t in self.threads if t["comments"]["nodes"]]
            return {"deletePullRequestReviewComment": {"pullRequestReview": {"id": self.pending_review}}}
        if op == "AddConversationComment":
            cid = f"IC_{len(self.conversation) + 2}"
            self.conversation.append({"__typename": "IssueComment", "id": cid, "body": v.get("body", ""),
                                      "createdAt": "2024-03-04T09:00:00Z",
                                      "url": f"{WEB}/pull/{PR_NUMBER}#issuecomment-{cid}",
                                      "author": {"login": "octocat", "avatarUrl": None}})
            return {"addComment": {"commentEdge": {"node": {"id": cid}}}}
        return {"createLinkedBranch": {"linkedBranch": {"id": "LB_stub"}}}


def _now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


class PullRequestEvents:
    """`354-pull-request-event-notifications`: other repositories' pull
    requests and what happens on them (see the module doc)."""

    def __init__(self):
        self.refuse = False
        self.version = 0
        self.prs: dict[tuple[str, str, int], dict] = {}
        self.reviews: dict[tuple[str, str, int], list[dict]] = {}
        self.comments: dict[int, dict] = {}
        self.threads: list[dict] = []
        # the stub's own origin: comment URLs elsewhere are never fetched
        self.base = "http://127.0.0.1"

    def _pr(self, owner: str, name: str, number: int, title: str, author: str, now: str, **extra) -> dict:
        web = f"http://127.0.0.1/{owner}/{name}"
        repo = {"name": name, "owner": _user(owner), "html_url": web, "clone_url": f"{web}.git",
                "default_branch": "main", "private": False, "fork": False, "parent": None}
        pr = {"number": number, "title": title, "state": "open", "draft": False, "created_at": now,
              "updated_at": now, "user": _user(author), "body": "", "assignees": [],
              "requested_reviewers": [], "requested_teams": [], "merged_at": None, "merged_by": None,
              "html_url": f"{web}/pull/{number}",
              "head": {"ref": f"topic-{number}", "sha": "a" * 40, "repo": repo},
              "base": {"ref": "main", "sha": "b" * 40, "repo": repo}}
        pr.update(extra)
        self.prs[(owner, name, number)] = pr
        return pr

    def _thread(self, reason: str, pr: dict, latest_comment: int | None = None) -> None:
        owner, name = pr["base"]["repo"]["owner"]["login"], pr["base"]["repo"]["name"]
        api = f"{self.base}/api/v3/repos/{owner}/{name}"
        self.threads.insert(0, {
            "id": str(len(self.threads) + 1), "reason": reason, "unread": True,
            "updated_at": pr["updated_at"],
            "subject": {"title": pr["title"], "type": "PullRequest", "url": f"{api}/pulls/{pr['number']}",
                        "latest_comment_url": f"{api}/issues/comments/{latest_comment}"
                        if latest_comment else f"{api}/pulls/{pr['number']}"},
            "repository": {"name": name, "full_name": f"{owner}/{name}", "owner": _user(owner)},
        })

    def happen(self, login: str = "octocat", base: int = 0) -> None:
        """Every event, dated now; `base` shifts the pull request numbers
        and ids, so a second round is new to Corvene."""
        now = _now()
        n = lambda number: number + base  # noqa: E731
        self._thread("review_requested", self._pr("octo-org", "website", n(21), "Redesign the landing page", "mona",
                                                  now, requested_reviewers=[_user(login)]))
        mine = self._pr("octo-org", "website", n(22), "Fix the footer links", login, now, state="closed",
                        merged_at=now, merged_by=_user("hubot"))
        self.reviews[("octo-org", "website", n(22))] = [
            {"id": n(7001), "user": _user("hubot"), "body": "Looks good!", "state": "APPROVED",
             "html_url": f"{mine['html_url']}#pullrequestreview-{n(7001)}", "submitted_at": now},
            {"id": n(7002), "user": _user(login), "body": "", "state": "PENDING",
             "html_url": f"{mine['html_url']}#pullrequestreview-{n(7002)}", "submitted_at": None},
        ]
        self._thread("author", mine)
        mention = self._pr("octo-org", "api", n(23), "Rate limit the search endpoint", "mona", now)
        self.comments[n(901)] = {"id": n(901), "body": f"@{login} could you check the limits?",
                                 "user": _user("mona"), "created_at": now,
                                 "html_url": f"{mention['html_url']}#issuecomment-{n(901)}",
                                 "pr": ("octo-org", "api", n(23))}
        self._thread("mention", mention, n(901))
        team = self._pr("octo-org", "api", n(24), "Bump the API version", "mona", now)
        self.comments[n(902)] = {"id": n(902), "body": "@octo-org/core please review", "user": _user("mona"),
                                 "created_at": now, "html_url": f"{team['html_url']}#issuecomment-{n(902)}",
                                 "pr": ("octo-org", "api", n(24))}
        self._thread("team_mention", team, n(902))
        self._thread("author", self._pr("octo-org", "api", n(25), "Tidy the changelog", login, now, state="closed",
                                        merged_at=now, merged_by=_user(login)))
        # the listed fixture repository: an approval of the account's pull request
        listed = self._pr(OWNER, NAME, n(32), "Explain the --login flag", login, now)
        self.reviews[(OWNER, NAME, n(32))] = [
            {"id": n(7003), "user": _user("mona"), "body": "Ship it.", "state": "APPROVED",
             "html_url": f"{listed['html_url']}#pullrequestreview-{n(7003)}", "submitted_at": now},
        ]
        self._thread("author", listed)
        self.version += 1

    def search(self, query: str, login: str) -> list[dict]:
        def item(key, pr):
            owner, name, _ = key
            return {"number": pr["number"], "title": pr["title"], "updated_at": pr["updated_at"],
                    "repository_url": f"http://stub/api/v3/repos/{owner}/{name}", "user": pr["user"]}
        out = []
        for key, pr in self.prs.items():
            if "review-requested:@me" in query:
                hit = pr["state"] == "open" and any(u["login"] == login for u in pr["requested_reviewers"])
            elif "author:@me" in query:
                hit = pr["user"]["login"] == login
            elif "mentions:@me" in query:
                hit = any(c["pr"] == key and f"@{login}" in c["body"] for c in self.comments.values())
            else:
                hit = False
            if hit:
                out.append(item(key, pr))
        return out

    def get(self, handler: "_Handler", path: str, params: dict, login: str) -> bool:
        """Answer `path` when it is one of these routes."""
        if path == "/api/v3/notifications":
            if self.refuse:
                handler._send(403, {"message": "Resource not accessible by personal access token"})
                return True
            headers = {"Last-Modified": f"stub-{self.version}", "X-Poll-Interval": "60"}
            if handler.headers.get("If-Modified-Since") == headers["Last-Modified"]:
                handler._send_empty(304, headers)
            else:
                handler._send(200, self.threads, headers)
            return True
        if path == "/api/v3/search/issues":
            items = self.search(urllib.parse.unquote_plus(params.get("q", "")), login)
            handler._send(200, {"total_count": len(items), "incomplete_results": False, "items": items})
            return True
        m = re.fullmatch(r"/api/v3/repos/([^/]+)/([^/]+)/pulls/(\d+)(/reviews)?", path)
        if m and (m[1], m[2], int(m[3])) in self.prs:
            key = (m[1], m[2], int(m[3]))
            handler._send(200, self.reviews.get(key, []) if m[4] else self.prs[key])
            return True
        m = re.fullmatch(r"/api/v3/repos/[^/]+/[^/]+/issues/comments/(\d+)", path)
        if m and int(m[1]) in self.comments:
            c = dict(self.comments[int(m[1])])
            c.pop("pr")
            handler._send(200, c)
            return True
        m = re.fullmatch(r"/api/v3/repos/([^/]+)/([^/]+)/issues/(\d+)/comments", path)
        if m and (m[1], m[2], int(m[3])) in self.prs:
            key = (m[1], m[2], int(m[3]))
            handler._send(200, [{k: v for k, v in c.items() if k != "pr"}
                                for c in self.comments.values() if c["pr"] == key])
            return True
        return False


class _Handler(BaseHTTPRequestHandler):
    server_version = "github-stub/1"

    def log_message(self, *_args):
        pass

    def _send(self, status: int, body, headers: dict | None = None) -> None:
        data = json.dumps(body).encode().replace(b"{STUB_BASE}", f"http://127.0.0.1:{self.server.port}".encode())
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("X-GitHub-Request-Id", "STUB:1")
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(data)

    def _send_empty(self, status: int, headers: dict | None = None) -> None:
        self.send_response(status)
        self.send_header("Content-Length", "0")
        self.send_header("X-GitHub-Request-Id", "STUB:1")
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()

    def _send_etag(self, body) -> None:
        """200 with an `ETag`, or 304 when `If-None-Match` matches it."""
        data = json.dumps(body).encode()
        etag = '"' + hashlib.md5(data).hexdigest() + '"'
        if self.headers.get("If-None-Match") == etag:
            self.server.not_modified += 1
            return self._send_empty(304)
        return self._send(200, body, {"ETag": etag})

    def _body(self) -> dict:
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            return json.loads(raw or b"{}")
        except json.JSONDecodeError:
            return {}

    def _login(self) -> str:
        """The account the request's token belongs to."""
        token = (self.headers.get("Authorization") or "").split(" ")[-1]
        return token[len("stub-token-"):] if token.startswith("stub-token-") else "octocat"

    def _fork(self, owner: str) -> dict:
        path = str(self.server.forks_dir / f"{NAME}-{owner}.git")
        return {"name": NAME, "owner": _user(owner), "html_url": f"http://127.0.0.1/{owner}/{NAME}",
                "clone_url": path, "ssh_url": path, "default_branch": "main", "private": False,
                "fork": True, "parent": None, "node_id": f"R_stub_{owner}"}

    def do_GET(self):  # noqa: N802
        path, _, query = self.path.partition("?")
        repo = f"/api/v3/repos/{OWNER}/{NAME}"
        params = dict(p.split("=", 1) for p in query.split("&") if "=" in p)
        login = self._login()
        self.server.requests.append((login, "GET", path))
        if path == "/api/v3/user":
            # the token's scopes: Corvene's sign-in, without `write:public_key`
            # (Settings › Integrations' Add to GitHub then asks to sign in again,
            # flag 350)
            name = "Mona Lisa Octocat" if login == "octocat" else f"{login} at work"
            return self._send(200, {**_user(login), "name": name, "plan": {"name": "free"}},
                              {"X-OAuth-Scopes": ", ".join(self.server.scopes)})
        if path == "/api/v3/user/orgs":
            return self._send(200, [])
        # `1314-code-owners`: octocat is on octo-org's rust team
        if path == "/api/v3/user/teams":
            teams = [{"slug": "rust", "name": "Rust", "organization": _user("octo-org")}]
            return self._send(200, teams if login == "octocat" else [])
        if self.server.pr_events.get(self, path, params, login):
            return
        if path == repo:
            perm = self.server.permissions.get(login, "admin")
            return self._send(200, {"name": NAME, "owner": _user(OWNER), "html_url": f"http://127.0.0.1/{OWNER}/{NAME}",
                                    "clone_url": f"http://127.0.0.1/{OWNER}/{NAME}.git", "default_branch": "main",
                                    "private": False, "fork": False, "parent": None, "node_id": "R_stub",
                                    "permissions": {"admin": perm == "admin", "push": perm in ("admin", "write"),
                                                    "pull": True}})
        if path == f"{repo}/issues":
            state = params.get("state", "open")
            issues = [i for i in self.server.created_issues + self.server.issues if state in ("all", i["state"])]
            return self._send(200, issues)
        m = re.fullmatch(rf"{re.escape(repo)}/issues/(\d+)", path)
        if m and (issue := self.server.issue(int(m[1]))):
            return self._send(200, issue)
        m = re.fullmatch(rf"{re.escape(repo)}/issues/(\d+)/comments", path)
        if m and self.server.issue(int(m[1])):
            comments = self.server.comments.get(int(m[1]), [])
            per_page = int(params.get("per_page", "30"))
            page = int(params.get("page", "1"))
            headers = {}
            if page * per_page < len(comments):
                headers["Link"] = (f'<http://127.0.0.1:{self.server.port}{path}?per_page={per_page}&page={page + 1}>; '
                                   'rel="next"')
            return self._send(200, comments[(page - 1) * per_page:page * per_page], headers)
        if path == "/assets/screenshot.png":
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(SCREENSHOT_PNG)))
            self.end_headers()
            self.wfile.write(SCREENSHOT_PNG)
            return None
        if path == f"{repo}/labels":
            return self._send(200, LABELS)
        # repository rules (`1225-branch-rules-link`): creating `release/*`
        # is restricted by a ruleset nobody bypasses; `hotfix/*` names must
        # end in a number by a ruleset the user always bypasses
        if path.startswith(f"{repo}/rules/branches/"):
            branch = urllib.parse.unquote(path[len(f"{repo}/rules/branches/"):])
            source = {"ruleset_source_type": "Repository", "ruleset_source": f"{OWNER}/{NAME}"}
            if branch.startswith("release/"):
                return self._send(200, [{"type": "creation", "ruleset_id": 41, **source}])
            if branch.startswith("hotfix/"):
                return self._send(200, [{"type": "branch_name_pattern", "ruleset_id": 42, **source,
                                         "parameters": {"name": "numbered", "negate": False,
                                                        "pattern": "^hotfix/[0-9]+$",
                                                        "operator": "regex"}}])
            return self._send(200, [])
        if path == f"{repo}/rulesets":
            return self._send(200, [{"id": 41, "name": "Releases"}, {"id": 42, "name": "Hotfixes"}])
        if path in (f"{repo}/rulesets/41", f"{repo}/rulesets/42"):
            rid = int(path.rsplit("/", 1)[1])
            return self._send(200, {"id": rid, "name": "Releases" if rid == 41 else "Hotfixes",
                                    "current_user_can_bypass": "never" if rid == 41 else "always"})
        if path == f"{repo}/assignees":
            return self._send(200, ASSIGNEES)
        if path == f"{repo}/releases":
            return self._send(200, self.server.created_releases + RELEASES)
        if path.startswith(f"{repo}/releases/tags/"):
            tag = path.rsplit("/", 1)[1]
            for r in self.server.created_releases + RELEASES:
                if r["tag_name"] == tag:
                    return self._send(200, r)
            return self._send(404, {"message": "Not Found"})
        pr = self.server.pull_request
        if pr is not None:
            if path == f"{repo}/pulls":
                return self._send(200, [pr.rest()] if params.get("state", "open") != "closed" else [])
            if path == f"{repo}/pulls/{PR_NUMBER}":
                return self._send(200, pr.rest())
        # CI (`334-branch-ci-status`, `347-actions-job-logs`): every ref has the
        # same three check runs of one Actions workflow run; job 101 failed
        if path.startswith(f"{repo}/commits/") and path.endswith("/status"):
            return self._send(200, {"state": "pending", "total_count": 0, "statuses": []})
        if path.startswith(f"{repo}/commits/") and path.endswith("/check-runs"):
            return self._send(200, {"total_count": len(CHECK_RUNS), "check_runs": CHECK_RUNS})
        # Actions (`351-actions`)
        if path == f"{repo}/actions/workflows":
            return self._send(200, {"total_count": len(WORKFLOWS), "workflows": WORKFLOWS})
        m = re.fullmatch(rf"{re.escape(repo)}/actions/(?:workflows/(\d+)/)?runs", path)
        if m:
            runs = self.server.runs
            if "check_suite_id" in params:
                # the check runs' workflow run (`334-branch-ci-status`)
                runs = [r for r in runs if str(r.get("check_suite_id")) == params["check_suite_id"]]
            if m.group(1):
                runs = [r for r in runs if r["workflow_id"] == int(m.group(1))]
            if params.get("branch"):
                runs = [r for r in runs if r["head_branch"] == _unquote(params["branch"])]
            if params.get("event"):
                runs = [r for r in runs if r["event"] == params["event"]]
            if params.get("status"):
                st = params["status"]
                runs = [r for r in runs if r["status"] == st or r["conclusion"] == st]
            per_page, page = int(params.get("per_page", 30)), int(params.get("page", 1))
            return self._send_etag({"total_count": len(runs),
                                    "workflow_runs": runs[(page - 1) * per_page:page * per_page]})
        m = re.fullmatch(rf"{re.escape(repo)}/actions/runs/(\d+)", path)
        if m:
            for r in self.server.runs:
                if r["id"] == int(m.group(1)):
                    return self._send(200, r)
            return self._send(404, {"message": "Not Found"})
        m = re.fullmatch(rf"{re.escape(repo)}/actions/runs/(\d+)/jobs", path)
        if m:
            jobs = JOBS if int(m.group(1)) == WORKFLOW_RUN["id"] else _jobs_for(self.server.run(int(m.group(1))))
            return self._send_etag({"total_count": len(jobs), "jobs": jobs})
        if path == f"{repo}/contents/.github/workflows/release.yml":
            data = RELEASE_YML.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return None
        if path == f"{repo}/contents/.github/workflows/ci.yml":
            data = CI_YML.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return None
        # `1223-checkout-from-fork`: alice's and bob's forks of the fixture
        if self.server.forks_dir is not None:
            forks = [self._fork(owner) for owner in FORK_OWNERS]
            if path == f"{repo}/forks":
                return self._send(200, forks)
            fork = next((f for f in forks if path == f"/api/v3/repos/{f['owner']['login']}/{NAME}"), None)
            if fork is not None:
                return self._send(200, fork)
        if path == f"{repo}/branches":
            return self._send(200, [{"name": "main", "protected": True}, {"name": "feature/login", "protected": False}])
        if path == f"{repo}/tags":
            return self._send(200, [{"name": "v0.1.0"}])
        if path == f"{repo}/environments":
            return self._send(200, {"total_count": 2, "environments": [{"name": "staging"}, {"name": "production"}]})
        if path.startswith(f"{repo}/actions/jobs/") and path.endswith("/logs"):
            job_id = int(path.rsplit("/", 2)[1])
            if job_id != 101:
                return self._send(404, {"message": "Not Found"})
            data = JOB_LOG.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return None
        return self._send(404, {"message": f"stub: no GET {path}"})

    def do_POST(self):  # noqa: N802
        path = self.path.partition("?")[0]
        repo = f"/api/v3/repos/{OWNER}/{NAME}"
        body = self._body()
        self.server.posts.append((path, body))
        self.server.requests.append((self._login(), "POST", path))
        if path == f"{repo}/issues":
            number = 101 + len(self.server.created_issues)
            issue = _issue(number, body.get("title", ""), "open", "2024-05-01T10:00:00Z",
                           [l for l in body.get("labels", []) if any(x["name"] == l for x in LABELS)],
                           body.get("assignees", []), 0, body.get("body", ""))
            self.server.created_issues.insert(0, issue)
            return self._send(201, issue)
        m = re.fullmatch(rf"{re.escape(repo)}/issues/(\d+)/comments", path)
        if m and (issue := self.server.issue(int(m[1]))):
            comments = self.server.comments.setdefault(issue["number"], [])
            cid = 9000 + len(self.server.posts)
            comment = _comment(cid, issue["number"], self._login(), "2024-05-02T10:00:00Z", body.get("body", ""))
            comments.append(comment)
            issue["comments"] = len(comments)
            return self._send(201, comment)
        if path == f"{repo}/releases/generate-notes":
            if os.environ.get("GITHUB_STUB_NO_NOTES") == "1":
                return self._send(404, {"message": "Not Found"})
            tag = body.get("tag_name", "")
            previous = body.get("previous_tag_name") or "v0.0.9"
            return self._send(200, {"name": f"{tag} generated", "body": (
                f"## What's Changed\n* Explain flags in the README by @octocat in #7\n* Add a logo by @octocat in #6\n\n"
                f"**Full Changelog**: http://127.0.0.1/{OWNER}/{NAME}/compare/{previous}...{tag}")})
        if path == f"{repo}/releases":
            rid = 9 + len(self.server.created_releases)
            release = _release(rid, body.get("tag_name", ""), body.get("name") or "", bool(body.get("draft")),
                               bool(body.get("prerelease")), "2024-05-01T10:00:00Z", assets=False)
            release["body"] = body.get("body", "")
            self.server.created_releases.insert(0, release)
            return self._send(201, release)
        # Actions (`351-actions`)
        m = re.fullmatch(rf"{re.escape(repo)}/actions/runs/(\d+)/(rerun|rerun-failed-jobs|cancel)", path)
        if m:
            run = self.server.run(int(m.group(1)))
            if run is None:
                return self._send(404, {"message": "Not Found"})
            if m.group(2) == "cancel":
                if run["status"] == "completed":
                    return self._send(409, {"message": "Cannot cancel a workflow run that is completed."})
                run.update(status="completed", conclusion="cancelled")
                return self._send(202, {})
            run.update(status="queued", conclusion=None, run_attempt=run["run_attempt"] + 1)
            return self._send(201, {})
        m = re.fullmatch(rf"{re.escape(repo)}/actions/jobs/(\d+)/rerun", path)
        if m:
            return self._send(201, {})
        m = re.fullmatch(rf"{re.escape(repo)}/actions/workflows/(\d+)/dispatches", path)
        if m:
            wid = int(m.group(1))
            if wid != 4:
                return self._send(422, {"message": "Workflow does not have 'workflow_dispatch' trigger"})
            unknown = [k for k in (body.get("inputs") or {}) if k not in ("version", "channel", "draft", "target")]
            if unknown:
                return self._send(422, {"message": f"Unexpected inputs provided: {unknown}"})
            now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            rid = 100 + len(self.server.runs)
            self.server.runs.insert(0, _run(rid, 4, "Release", "Release", 13 + rid - 100, "queued", None,
                                            body.get("ref", "main"), "workflow_dispatch", now, None))
            return self._send_empty(204)
        if path == "/api/graphql":
            pr = self.server.pull_request
            if pr is None:
                return self._send(200, {"data": {"createLinkedBranch": {"linkedBranch": {"id": "LB_stub"}}}})
            return self._send(200, {"data": pr.graphql(body.get("query", ""), body.get("variables") or {})})
        if path == "/api/v3/user/keys":
            if "write:public_key" not in self.server.scopes:
                return self._send(404, {"message": "Not Found"})
            return self._send(201, {"id": 1, "title": body.get("title", ""), "key": body.get("key", "")})
        return self._send(404, {"message": f"stub: no POST {path}"})


def _delete(self):  # noqa: N802
    path = self.path.partition("?")[0]
    repo = f"/api/v3/repos/{OWNER}/{NAME}"
    self.server.posts.append((f"DELETE {path}", {}))
    m = re.fullmatch(rf"{re.escape(repo)}/actions/runs/(\d+)/logs", path)
    if m and self.server.run(int(m.group(1))) is not None:
        return self._send_empty(204)
    return self._send(404, {"message": f"stub: no DELETE {path}"})


_Handler.do_DELETE = _delete


def _patch(self):  # noqa: N802
    """`345-issues`: state, labels and assignees of an issue."""
    path = self.path.partition("?")[0]
    repo = f"/api/v3/repos/{OWNER}/{NAME}"
    body = self._body()
    self.server.posts.append((f"PATCH {path}", body))
    self.server.requests.append((self._login(), "PATCH", path))
    m = re.fullmatch(rf"{re.escape(repo)}/issues/(\d+)", path)
    issue = self.server.issue(int(m[1])) if m else None
    if issue is None:
        return self._send(404, {"message": f"stub: no PATCH {path}"})
    if "state" in body:
        issue["state"] = body["state"]
        issue["closed_at"] = "2024-05-02T10:00:00Z" if body["state"] == "closed" else None
        issue["state_reason"] = body.get("state_reason") or ("completed" if body["state"] == "closed" else "reopened")
    if "labels" in body:
        by_name = {l["name"]: l for l in LABELS}
        issue["labels"] = [by_name[n] for n in body["labels"] if n in by_name]
    if "assignees" in body:
        issue["assignees"] = [_user(a) for a in body["assignees"]]
    issue["updated_at"] = "2024-05-02T10:00:00Z"
    return self._send(200, issue)


def _lock(self, locked: bool):
    path = self.path.partition("?")[0]
    repo = f"/api/v3/repos/{OWNER}/{NAME}"
    self.server.posts.append((f"{'PUT' if locked else 'DELETE'} {path}", {}))
    m = re.fullmatch(rf"{re.escape(repo)}/issues/(\d+)/lock", path)
    issue = self.server.issue(int(m[1])) if m else None
    if issue is None:
        return False
    issue["locked"] = locked
    issue["active_lock_reason"] = None
    self._send_empty(204)
    return True


def _put(self):  # noqa: N802
    self._body()
    if not _lock(self, True):
        self._send(404, {"message": f"stub: no PUT {self.path}"})


def _delete_or_unlock(self):  # noqa: N802
    if not _lock(self, False):
        _delete(self)


_Handler.do_PATCH = _patch
_Handler.do_PUT = _put
_Handler.do_DELETE = _delete_or_unlock


def _unquote(text: str) -> str:
    from urllib.parse import unquote
    return unquote(text)


def _check_run(run_id: int, name: str, conclusion: str, started: str, completed: str) -> dict:
    return {"id": run_id, "name": name, "status": "completed", "conclusion": conclusion,
            "check_suite": {"id": 11}, "app": {"name": "GitHub Actions", "slug": "github-actions"},
            "started_at": started, "completed_at": completed,
            "html_url": f"http://127.0.0.1/{OWNER}/{NAME}/actions/runs/7/job/{run_id}", "pull_requests": []}


CHECK_RUNS = [
    _check_run(101, "test (macos-15)", "failure", "2024-05-01T10:00:00Z", "2024-05-01T10:05:10Z"),
    _check_run(102, "lint", "success", "2024-05-01T10:00:00Z", "2024-05-01T10:01:34Z"),
    _check_run(103, "build (ubuntu-latest)", "success", "2024-05-01T10:00:00Z", "2024-05-01T10:03:02Z"),
]

def _run(rid: int, workflow_id: int, name: str, title: str, number: int, status: str, conclusion: str | None,
         branch: str, event: str, created: str, updated: str | None, suite: int | None = None) -> dict:
    return {"id": rid, "workflow_id": workflow_id, "name": name, "display_title": title, "run_number": number,
            "run_attempt": 1, "status": status, "conclusion": conclusion, "head_branch": branch,
            "head_sha": f"{rid:04d}a1c0d5e7b9a3f1c2d4e6f8a0b1c3d5e7f9a", "event": event,
            "actor": _user("octocat"), "triggering_actor": _user("octocat"),
            "html_url": f"http://127.0.0.1/{OWNER}/{NAME}/actions/runs/{rid}", "created_at": created,
            "updated_at": updated or created, "run_started_at": created, "check_suite_id": suite,
            "path": ".github/workflows/release.yml" if workflow_id == 4 else ".github/workflows/ci.yml"}


WORKFLOWS = [
    {"id": 3, "name": "CI", "path": ".github/workflows/ci.yml", "state": "active",
     "html_url": f"http://127.0.0.1/{OWNER}/{NAME}/blob/main/.github/workflows/ci.yml"},
    {"id": 4, "name": "Release", "path": ".github/workflows/release.yml", "state": "active",
     "html_url": f"http://127.0.0.1/{OWNER}/{NAME}/blob/main/.github/workflows/release.yml"},
]

WORKFLOW_RUN = _run(7, 3, "CI", "Explain flags in the README", 21, "completed", "failure", "feature/login",
                    "pull_request", "2024-05-01T09:59:50Z", "2024-05-01T10:05:10Z", suite=11)


def _runs() -> list[dict]:
    """The runs, newest first (fresh copies: actions change them)."""
    return [
        _run(9, 3, "CI", "Add a logo", 23, "in_progress", None, "main", "push", "2024-05-03T10:00:00Z", None),
        _run(8, 4, "Release", "Release", 12, "completed", "success", "main", "workflow_dispatch",
             "2024-05-02T10:00:00Z", "2024-05-02T10:20:54Z"),
        json.loads(json.dumps(WORKFLOW_RUN)),
        _run(6, 3, "CI", "Initial commit", 20, "completed", "cancelled", "main", "push",
             "2024-04-30T10:00:00Z", "2024-04-30T10:01:15Z"),
        _run(5, 3, "CI", "Initial commit", 19, "completed", "success", "main", "schedule",
             "2024-04-29T03:00:00Z", "2024-04-29T03:04:02Z"),
    ]


def _jobs_for(run: dict | None) -> list[dict]:
    """One job for a run other than 7, in the run's state."""
    if run is None:
        return []
    done = run["status"] == "completed"
    return [{"id": 200 + run["id"], "name": "build", "status": run["status"], "conclusion": run["conclusion"],
             "html_url": f"{run['html_url']}/job/{200 + run['id']}",
             "started_at": run["created_at"], "completed_at": run["updated_at"] if done else None, "steps": [
                 _step(1, "Set up job", "success", run["created_at"], run["created_at"]),
                 {"name": "cargo build", "number": 2, "status": run["status"], "conclusion": run["conclusion"],
                  "started_at": run["created_at"], "completed_at": run["updated_at"] if done else None}]}]


CI_YML = """name: CI
on:
  push:
  pull_request:
  schedule:
    - cron: '0 3 * * *'
jobs:
  test:
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v4
"""

RELEASE_YML = """name: Release
on:
  workflow_dispatch:
    inputs:
      version:
        description: Version to release
        required: true
        type: string
      channel:
        description: Channel
        type: choice
        options: [stable, beta]
        default: beta
      draft:
        description: Create the release as a draft
        type: boolean
        default: true
      target:
        description: Deploy to
        type: environment
jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - run: echo releasing
"""


def _step(number: int, name: str, conclusion: str, started: str, completed: str) -> dict:
    return {"name": name, "number": number, "status": "completed", "conclusion": conclusion,
            "started_at": started, "completed_at": completed}


JOBS = [
    {"id": 101, "name": "test (macos-15)", "status": "completed", "conclusion": "failure",
     "started_at": "2024-05-01T10:00:00Z", "completed_at": "2024-05-01T10:05:10Z",
     "html_url": CHECK_RUNS[0]["html_url"], "steps": [
         _step(1, "Set up job", "success", "2024-05-01T10:00:00Z", "2024-05-01T10:00:02Z"),
         _step(2, "Checkout", "success", "2024-05-01T10:00:02Z", "2024-05-01T10:00:05Z"),
         _step(3, "cargo clippy", "success", "2024-05-01T10:00:05Z", "2024-05-01T10:01:39Z"),
         _step(4, "cargo test --workspace", "failure", "2024-05-01T10:01:39Z", "2024-05-01T10:05:10Z"),
         _step(5, "Post Checkout", "skipped", "2024-05-01T10:05:10Z", "2024-05-01T10:05:10Z")]},
    {"id": 102, "name": "lint", "status": "completed", "conclusion": "success",
     "started_at": "2024-05-01T10:00:00Z", "completed_at": "2024-05-01T10:01:34Z",
     "html_url": CHECK_RUNS[1]["html_url"], "steps": [
         _step(1, "Set up job", "success", "2024-05-01T10:00:00Z", "2024-05-01T10:00:02Z"),
         _step(2, "cargo fmt --check", "success", "2024-05-01T10:00:02Z", "2024-05-01T10:01:34Z")]},
    {"id": 103, "name": "build (ubuntu-latest)", "status": "completed", "conclusion": "success",
     "started_at": "2024-05-01T10:00:00Z", "completed_at": "2024-05-01T10:03:02Z",
     "html_url": CHECK_RUNS[2]["html_url"], "steps": [
         _step(1, "Set up job", "success", "2024-05-01T10:00:00Z", "2024-05-01T10:00:02Z"),
         _step(2, "cargo build", "success", "2024-05-01T10:00:02Z", "2024-05-01T10:03:02Z")]},
]

# the failed job's log, as `GET /actions/jobs/101/logs` serves it (timestamps,
# `##[group]` / `##[error]` commands and ANSI colours)
_LOG_LINES = [
    (0, "Current runner version: '2.317.0'"),
    (0, "##[group]Operating System"), (0, "macOS"), (0, "15.0"), (0, "##[endgroup]"),
    (1, "##[group]Run actions/checkout@v4"), (1, "with:"), (1, "  fetch-depth: 0"), (1, "##[endgroup]"),
    (2, "Syncing repository: octocat/parity-fixture"),
    (3, "##[group]Run cargo clippy --all-targets -- -D warnings"),
    (3, "\x1b[36;1mcargo clippy --all-targets -- -D warnings\x1b[0m"), (3, "shell: /bin/bash -e {0}"),
    (3, "##[endgroup]"),
    (10, "\x1b[1m\x1b[32m    Checking\x1b[0m corvene-core v0.1.0"),
    (97, "\x1b[1m\x1b[32m    Finished\x1b[0m `dev` profile [unoptimized + debuginfo] target(s) in 1m 34s"),
    (98, "##[group]Run cargo test --workspace"), (98, "\x1b[36;1mcargo test --workspace\x1b[0m"),
    (98, "shell: /bin/bash -e {0}"), (98, "##[endgroup]"),
    (140, "\x1b[1m\x1b[32m     Running\x1b[0m unittests src/lib.rs (target/debug/deps/corvene_core-8f2a1c)"),
    (141, "running 412 tests"),
    (300, "test job_log::tests::searches_case_insensitively ... \x1b[32mok\x1b[0m"),
    (301, "test remote::tests::fetch_skips_unchanged ... \x1b[31mFAILED\x1b[0m"),
    (302, "failures:"), (302, "---- remote::tests::fetch_skips_unchanged stdout ----"),
    (302, "thread 'remote::tests::fetch_skips_unchanged' panicked at crates/corvene-core/src/remote.rs:2710:9:"),
    (302, "assertion `left == right` failed"), (302, "  left: 2"), (302, " right: 1"),
    (302, "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"),
    (303, "test result: \x1b[31mFAILED\x1b[0m. 411 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 162.03s"),
    (309, "\x1b[1m\x1b[31merror\x1b[0m: test failed, to rerun pass `-p corvene-core --lib`"),
    (309, "##[error]Process completed with exit code 101."),
    (310, "Post job cleanup."), (310, "##[group]Run actions/checkout@v4"), (310, "##[endgroup]"),
    (310, "Cleaning up orphan processes"),
]
JOB_LOG = "".join(f"2024-05-01T10:{s // 60:02}:{s % 60:02}.0000000Z {text}\n" for s, text in _LOG_LINES)


class Stub(HTTPServer):
    def __init__(self, repo: Path | None = None, forks_dir: Path | None = None):
        super().__init__(("127.0.0.1", 0), _Handler)
        # `1223-checkout-from-fork`: where alice's and bob's forks are
        self.forks_dir = forks_dir
        self.created_issues: list[dict] = []
        # `345-issues`: the fixture issues and their comments, as writes leave them
        self.issues: list[dict] = json.loads(json.dumps(OPEN_ISSUES + CLOSED_ISSUES))
        self.comments: dict[int, list[dict]] = _comments()
        self.created_releases: list[dict] = []
        self.posts: list[tuple[str, dict]] = []
        # `527-multiple-accounts`: (login, method, path) of every request,
        # and what each login may do with the repository (admin by default)
        self.requests: list[tuple[str, str, str]] = []
        self.permissions: dict[str, str] = {}
        # `351-actions`: the runs actions change, and 304 answers given
        self.runs: list[dict] = _runs()
        self.not_modified = 0
        # `348-pull-request-review`: pull request #7 over the fixture
        self.pull_request = PullRequestFixture(repo) if repo else None
        # `354-pull-request-event-notifications`
        self.pr_events = PullRequestEvents()
        self.pr_events.base = f"http://127.0.0.1:{self.server_address[1]}"
        self.scopes: list[str] = os.environ.get(
            "GITHUB_STUB_SCOPES", "repo workflow read:user user:email").split()
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)

    def issue(self, number: int) -> dict | None:
        return next((i for i in self.created_issues + self.issues if i["number"] == number), None)

    def run(self, rid: int) -> dict | None:
        return next((r for r in self.runs if r["id"] == rid), None)

    @property
    def port(self) -> int:
        return self.server_address[1]

    def hook_arg(self, extra_logins: list[str] | None = None) -> str:
        """The `fake-github` control hook's argument; `extra_logins` sign in
        more accounts on the stub (`527-multiple-accounts`)."""
        arg = {"port": self.port, "owner": OWNER, "name": NAME, "login": "octocat"}
        if extra_logins:
            arg["extra_logins"] = extra_logins
        return json.dumps(arg)

    def start(self) -> "Stub":
        self.thread.start()
        return self

    def stop(self) -> None:
        self.shutdown()
        self.server_close()


def start(repo: Path | None = None, forks_dir: Path | None = None) -> Stub:
    return Stub(repo, forks_dir).start()


if __name__ == "__main__":
    stub = start()
    print(f"github stub on http://127.0.0.1:{stub.port}/api/v3")
    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        stub.stop()
