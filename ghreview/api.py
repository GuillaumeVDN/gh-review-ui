"""GitHub domain API — PRs, files, diffs, viewed-state, and reviews.

Built on :mod:`ghreview.gh`. These functions block on the network and are meant
to run inside the worker thread.
"""
import json
import os

from .gh import gh_json, gh_graphql, sh
from .diff import parse_diff
from .models import PR, Commit, FileEntry, PendingComment

# Lines of context shown around each hunk (git's own default is 3).
DIFF_CONTEXT = 8


# ---- repo / identity ----

def detect_repo():
    d = gh_json(["repo", "view", "--json", "owner,name"])
    return d["owner"]["login"], d["name"]


def get_viewer_login():
    try:
        return gh_json(["api", "user"]).get("login", "")
    except Exception:
        return ""


def get_repo_root():
    try:
        return sh(["git", "rev-parse", "--show-toplevel"]).strip()
    except Exception:
        return os.getcwd()


# ---- PRs / files / diff ----

def load_prs():
    fields = "number,title,headRefName,author,id"
    # (search, category): authored first so a PR I both own and was asked to
    # review is filed under "mine".
    searches = [
        ("is:open author:@me", "mine"),
        ("is:open review-requested:@me", "review"),
        ("is:open reviewed-by:@me", "review"),
    ]
    seen = {}
    for search, category in searches:
        try:
            data = gh_json(["pr", "list", "--limit", "50", "--search", search, "--json", fields])
        except Exception:
            data = []
        for p in data:
            if p["number"] in seen:
                continue
            seen[p["number"]] = PR(
                number=p["number"], title=p["title"], head=p["headRefName"],
                author=p.get("author", {}).get("login", "?"), node_id=p["id"],
                category=category,
            )
    # "mine" group first, then "review"; newest PR first within each group.
    return sorted(seen.values(), key=lambda pr: (0 if pr.category == "mine" else 1, -pr.number))


def load_files(owner, name, number):
    q = """
    query($owner:String!, $name:String!, $number:Int!, $after:String) {
      repository(owner:$owner, name:$name) {
        pullRequest(number:$number) {
          id
          files(first:100, after:$after) {
            nodes { path viewerViewedState }
            pageInfo { hasNextPage endCursor }
          }
        }
      }
    }
    """
    entries = []
    pr_id = ""
    after = None
    while True:
        data = gh_graphql(q, owner=owner, name=name, number=number, after=after)
        pr = data["data"]["repository"]["pullRequest"]
        pr_id = pr["id"]
        for n in pr["files"]["nodes"]:
            entries.append(FileEntry(path=n["path"], viewed=n["viewerViewedState"] == "VIEWED"))
        pi = pr["files"]["pageInfo"]
        if not pi["hasNextPage"]:
            break
        after = pi["endCursor"]
    return pr_id, entries


def load_diff(number):
    raw = sh(["gh", "pr", "diff", str(number)])
    return parse_diff(raw)


def load_commits(number):
    """Commits of the PR, newest first (like ``git log``)."""
    d = gh_json([
        "pr", "view", str(number),
        "--json", "commits",
    ])
    out = []
    for c in d.get("commits", []):
        authors = c.get("authors") or []
        author = (authors[0].get("login") or authors[0].get("name")) if authors else ""
        out.append(Commit(
            oid=c.get("oid", ""),
            headline=c.get("messageHeadline", ""),
            body=c.get("messageBody", "") or "",
            author=author or "",
            date=c.get("authoredDate", "") or "",
        ))
    out.reverse()  # gh returns oldest first; show newest at the top
    return out


def load_diff_range(first_oid, last_oid):
    """Cumulative diff from the parent of ``first_oid`` through ``last_oid``.

    A single commit (``first_oid == last_oid``) yields just that commit's diff.
    Requires the PR to be checked out locally so the commits resolve.
    """
    raw = sh(["git", "diff", f"-U{DIFF_CONTEXT}", f"{first_oid}^..{last_oid}"])
    return parse_diff(raw)


def load_pr_details(number):
    return gh_json([
        "pr", "view", str(number),
        "--json", "title,body,author,state,url,createdAt,reviewDecision,comments,reviews",
    ])


# ---- worktrees ----
#
# Instead of `gh pr checkout` (which switches the main checkout's branch), each
# PR is materialised in its own git worktree under the cache dir. The main repo
# stays on whatever branch you're working on; the worktree holds the PR head so
# you (or agents) can edit the reviewed code independently.

def base_remote(repo_root, owner, name):
    """The git remote that points at ``owner/name`` (falls back to origin)."""
    try:
        out = sh(["git", "-C", repo_root, "remote", "-v"])
    except Exception:
        return "origin"
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 2 and f"{owner}/{name}" in parts[1] and "fetch" in line:
            return parts[0]
    return "origin"


def worktree_path(owner, name, number):
    """Stable on-disk location for a PR's review worktree."""
    cache = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
    return os.path.join(cache, "gh-review-ui", "worktrees",
                        f"{owner}__{name}", f"pr-{number}")


def _is_worktree(path):
    if not os.path.isdir(path):
        return False
    try:
        sh(["git", "-C", path, "rev-parse", "--is-inside-work-tree"])
        return True
    except Exception:
        return False


def open_pr_worktree(repo_root, owner, name, number):
    """Fetch the PR head and check it out in a dedicated worktree.

    Returns the worktree path. Reuses/refreshes an existing worktree so
    repeatedly opening (or refreshing) a PR is cheap and picks up new pushes.
    """
    remote = base_remote(repo_root, owner, name)
    ref = f"refs/gh-review-ui/pr-{number}"
    branch = f"gh-review-ui/pr-{number}"
    sh(["git", "-C", repo_root, "fetch", remote, f"+refs/pull/{number}/head:{ref}"])
    sha = sh(["git", "-C", repo_root, "rev-parse", ref]).strip()
    path = worktree_path(owner, name, number)
    if _is_worktree(path):
        sh(["git", "-C", path, "checkout", "-B", branch, sha])
    else:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        sh(["git", "-C", repo_root, "worktree", "prune"])
        sh(["git", "-C", repo_root, "worktree", "add", "--force", "-B", branch, path, sha])
    return path


# ---- session persistence (remember the last-opened PR per repo) ----

def _session_file():
    cache = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
    return os.path.join(cache, "gh-review-ui", "last-pr.json")


def save_last_pr(owner, name, number):
    """Remember the PR most recently opened for ``owner/name``."""
    path = _session_file()
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        data = {}
        if os.path.exists(path):
            with open(path) as f:
                data = json.load(f)
        data[f"{owner}/{name}"] = number
        with open(path, "w") as f:
            json.dump(data, f)
    except Exception:
        pass


def load_last_pr(owner, name):
    """Return the last-opened PR number for ``owner/name``, or None."""
    try:
        with open(_session_file()) as f:
            return json.load(f).get(f"{owner}/{name}")
    except Exception:
        return None


# ---- viewed state ----

def mark_viewed_api(pr_id, path, viewed):
    m = "markFileAsViewed" if viewed else "unmarkFileAsViewed"
    q = (f"mutation($pr:ID!, $path:String!) {{ {m}(input:{{pullRequestId:$pr, "
         f"path:$path}}) {{ pullRequest {{ id }} }} }}")
    gh_graphql(q, pr=pr_id, path=path)


def mark_viewed_bulk_api(pr_id, paths, viewed, chunk=100):
    """Mark/unmark many files in one GraphQL request per chunk.

    There's no multi-path mutation, but aliasing lets us pack many
    (un)markFileAsViewed calls into a single request. Returns ``(done, errs)``;
    a failed batch falls back to per-file so one bad path doesn't sink the rest.
    """
    mutation = "markFileAsViewed" if viewed else "unmarkFileAsViewed"
    done, errs = [], []
    for i in range(0, len(paths), chunk):
        batch = paths[i:i + chunk]
        decls = ["$pr:ID!"] + [f"$p{j}:String!" for j in range(len(batch))]
        body = "\n".join(
            f"  f{j}: {mutation}(input:{{pullRequestId:$pr, path:$p{j}}}) "
            f"{{ clientMutationId }}"
            for j in range(len(batch))
        )
        q = f"mutation({', '.join(decls)}) {{\n{body}\n}}"
        variables = {"pr": pr_id}
        for j, p in enumerate(batch):
            variables[f"p{j}"] = p
        try:
            gh_graphql(q, **variables)
            done.extend(batch)
        except Exception:
            for p in batch:
                try:
                    mark_viewed_api(pr_id, p, viewed)
                    done.append(p)
                except Exception as e:
                    errs.append((p, str(e)))
    return done, errs


# ---- pending review ----

def find_pending_review(owner, name, number, login):
    """Return ``(pr_id, pending_review_id_or_None)`` for the viewer's draft."""
    q = """
    query($owner:String!, $name:String!, $number:Int!, $login:String!) {
      repository(owner:$owner, name:$name) {
        pullRequest(number:$number) {
          id
          reviews(first:1, author:$login, states:[PENDING]) { nodes { id } }
        }
      }
    }
    """
    d = gh_graphql(q, owner=owner, name=name, number=number, login=login)
    pr = d["data"]["repository"]["pullRequest"]
    nodes = pr["reviews"]["nodes"]
    return pr["id"], (nodes[0]["id"] if nodes else None)


def _ensure_pending_review(owner, name, number, login, pr_id):
    _, review_id = find_pending_review(owner, name, number, login)
    if review_id:
        return review_id
    q = """
    mutation($pr:ID!) {
      addPullRequestReview(input:{pullRequestId:$pr}) { pullRequestReview { id } }
    }
    """
    return gh_graphql(q, pr=pr_id)["data"]["addPullRequestReview"]["pullRequestReview"]["id"]


def load_pending_comments(owner, name, number, login):
    """Return the viewer's not-yet-submitted (pending) review comments."""
    q = """
    query($owner:String!, $name:String!, $number:Int!, $login:String!) {
      repository(owner:$owner, name:$name) {
        pullRequest(number:$number) {
          reviews(first:10, author:$login, states:[PENDING]) {
            nodes {
              comments(first:100) {
                nodes { id path body line originalLine }
              }
            }
          }
        }
      }
    }
    """
    d = gh_graphql(q, owner=owner, name=name, number=number, login=login)
    reviews = d["data"]["repository"]["pullRequest"]["reviews"]["nodes"]
    out = []
    for r in reviews:
        for c in r["comments"]["nodes"]:
            line = c.get("line") or c.get("originalLine") or 0
            out.append(PendingComment(path=c["path"], body=c.get("body") or "",
                                      line=line, side="RIGHT", comment_id=c["id"]))
    return out


def add_pending_comment_api(owner, name, number, login, pr_id, comment):
    """Ensure a pending review exists, then add ``comment`` as a thread to it.

    Includes ``startLine``/``startSide`` for a multi-line (range) comment.
    """
    review_id = _ensure_pending_review(owner, name, number, login, pr_id)
    decls = "$r:ID!, $path:String!, $line:Int!, $body:String!, $side:DiffSide!"
    fields = ("pullRequestReviewId:$r, path:$path, line:$line, body:$body, "
              "side:$side")
    variables = dict(r=review_id, path=comment.path, line=comment.line,
                     body=comment.body, side=comment.side)
    if comment.start_line is not None and comment.start_side:
        decls += ", $startLine:Int!, $startSide:DiffSide!"
        fields += ", startLine:$startLine, startSide:$startSide"
        variables["startLine"] = comment.start_line
        variables["startSide"] = comment.start_side
    q = (f"mutation({decls}) {{ addPullRequestReviewThread(input:{{{fields}}}) "
         f"{{ thread {{ id }} }} }}")
    gh_graphql(q, **variables)


def delete_pending_comment_api(comment_id):
    q = """
    mutation($id:ID!) {
      deletePullRequestReviewComment(input:{id:$id}) { clientMutationId }
    }
    """
    gh_graphql(q, id=comment_id)


def update_pending_comment_api(comment_id, body):
    q = """
    mutation($id:ID!, $body:String!) {
      updatePullRequestReviewComment(input:{pullRequestReviewCommentId:$id, body:$body}) {
        pullRequestReviewComment { id }
      }
    }
    """
    gh_graphql(q, id=comment_id, body=body)


def submit_review_api(owner, name, number, login, pr_id, event, body):
    """Submit the viewer's pending review (creating an empty one if needed)."""
    review_id = _ensure_pending_review(owner, name, number, login, pr_id)
    q = """
    mutation($r:ID!, $event:PullRequestReviewEvent!, $body:String) {
      submitPullRequestReview(input:{pullRequestReviewId:$r, event:$event, body:$body}) {
        pullRequestReview { id }
      }
    }
    """
    gh_graphql(q, r=review_id, event=event, body=body or None)
