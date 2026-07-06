"""GitHub domain API — PRs, files, diffs, viewed-state, and reviews.

Built on :mod:`ghreview.gh`. These functions block on the network and are meant
to run inside the worker thread.
"""
import os

from .gh import gh_json, gh_graphql, sh
from .diff import parse_diff
from .models import PR, FileEntry, PendingComment


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
    seen = {}
    for search in ("is:open author:@me", "is:open review-requested:@me"):
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
            )
    return sorted(seen.values(), key=lambda pr: -pr.number)


def current_pr_number():
    try:
        return gh_json(["pr", "view", "--json", "number"])["number"]
    except Exception:
        return None


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


def load_pr_details(number):
    return gh_json([
        "pr", "view", str(number),
        "--json", "title,body,author,state,url,createdAt,reviewDecision,comments,reviews",
    ])


def checkout_pr(number):
    sh(["gh", "pr", "checkout", str(number)])


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
    """Ensure a pending review exists, then add ``comment`` as a thread to it."""
    review_id = _ensure_pending_review(owner, name, number, login, pr_id)
    q = """
    mutation($r:ID!, $path:String!, $line:Int!, $body:String!, $side:DiffSide!) {
      addPullRequestReviewThread(input:{
        pullRequestReviewId:$r, path:$path, line:$line, body:$body, side:$side
      }) { thread { id } }
    }
    """
    gh_graphql(q, r=review_id, path=comment.path, line=comment.line,
               body=comment.body, side=comment.side)


def delete_pending_comment_api(comment_id):
    q = """
    mutation($id:ID!) {
      deletePullRequestReviewComment(input:{id:$id}) { clientMutationId }
    }
    """
    gh_graphql(q, id=comment_id)


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
