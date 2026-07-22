import pytest

from ghreview import api


class FakeGraphQL:
    """Records calls; optionally raises for queries matching a predicate."""
    def __init__(self, responses=None, raise_if=None):
        self.calls = []
        self.responses = responses or {}
        self.raise_if = raise_if

    def __call__(self, query, **variables):
        self.calls.append((query, variables))
        if self.raise_if and self.raise_if(query):
            raise RuntimeError("boom")
        for needle, resp in self.responses.items():
            if needle in query:
                return resp
        return {}


def test_bulk_mark_one_request_for_many(monkeypatch):
    fake = FakeGraphQL()
    monkeypatch.setattr(api, "gh_graphql", fake)
    done, errs = api.mark_viewed_bulk_api("PR", ["a.py", "b/c.py", "d.py"], True)
    assert done == ["a.py", "b/c.py", "d.py"]
    assert errs == []
    assert len(fake.calls) == 1  # single aliased request
    query, variables = fake.calls[0]
    assert query.count("markFileAsViewed") == 3
    assert variables == {"pr": "PR", "p0": "a.py", "p1": "b/c.py", "p2": "d.py"}


def test_bulk_mark_chunks(monkeypatch):
    fake = FakeGraphQL()
    monkeypatch.setattr(api, "gh_graphql", fake)
    paths = [f"f{i}.py" for i in range(5)]
    done, errs = api.mark_viewed_bulk_api("PR", paths, False, chunk=2)
    assert done == paths and errs == []
    assert len(fake.calls) == 3  # 2 + 2 + 1


def test_bulk_mark_falls_back_per_file_on_batch_error(monkeypatch):
    # Batch (has an "f0:" alias) fails; single-file mutations succeed.
    fake = FakeGraphQL(raise_if=lambda q: "f0:" in q)
    monkeypatch.setattr(api, "gh_graphql", fake)
    done, errs = api.mark_viewed_bulk_api("PR", ["a.py", "b.py"], True)
    assert set(done) == {"a.py", "b.py"}
    assert errs == []


def test_unmark_uses_unmark_mutation(monkeypatch):
    fake = FakeGraphQL()
    monkeypatch.setattr(api, "gh_graphql", fake)
    api.mark_viewed_bulk_api("PR", ["a.py"], False)
    assert "unmarkFileAsViewed" in fake.calls[0][0]


def test_load_pending_comments_parses_nodes(monkeypatch):
    resp = {"data": {"repository": {"pullRequest": {"reviews": {"nodes": [
        {"comments": {"nodes": [
            {"id": "C1", "path": "a.py", "body": "nit", "line": 12, "originalLine": None},
            {"id": "C2", "path": "b.py", "body": "", "line": None, "originalLine": 3},
        ]}},
    ]}}}}}
    monkeypatch.setattr(api, "gh_graphql", FakeGraphQL(responses={"reviews": resp}))
    out = api.load_pending_comments("o", "n", 1, "me")
    assert [(c.path, c.line, c.comment_id) for c in out] == [
        ("a.py", 12, "C1"), ("b.py", 3, "C2")]


def test_find_pending_review_returns_id(monkeypatch):
    resp = {"data": {"repository": {"pullRequest": {
        "id": "PR", "reviews": {"nodes": [{"id": "REV"}]}}}}}
    monkeypatch.setattr(api, "gh_graphql", FakeGraphQL(responses={"reviews": resp}))
    assert api.find_pending_review("o", "n", 1, "me") == ("PR", "REV")


def test_add_pending_reuses_existing_review(monkeypatch):
    from ghreview.models import PendingComment
    found = {"data": {"repository": {"pullRequest": {
        "id": "PR", "reviews": {"nodes": [{"id": "REV"}]}}}}}
    fake = FakeGraphQL(responses={"reviews": found, "addPullRequestReviewThread": {}})
    monkeypatch.setattr(api, "gh_graphql", fake)
    api.add_pending_comment_api("o", "n", 1, "me", "PR",
                                PendingComment("a.py", "hi", 5, "RIGHT"))
    # no new review created (reused REV), thread added to it
    assert not any("addPullRequestReview(" in q for q, _ in fake.calls)
    thread_calls = [v for q, v in fake.calls if "addPullRequestReviewThread" in q]
    assert thread_calls and thread_calls[0]["r"] == "REV"


def test_submit_creates_review_when_none(monkeypatch):
    none_found = {"data": {"repository": {"pullRequest": {
        "id": "PR", "reviews": {"nodes": []}}}}}
    created = {"data": {"addPullRequestReview": {"pullRequestReview": {"id": "NEW"}}}}
    fake = FakeGraphQL(responses={"reviews": none_found,
                                  "addPullRequestReview(": created,
                                  "submitPullRequestReview": {}})
    monkeypatch.setattr(api, "gh_graphql", fake)
    api.submit_review_api("o", "n", 1, "me", "PR", "APPROVE", "lgtm")
    submits = [v for q, v in fake.calls if "submitPullRequestReview" in q]
    assert submits and submits[0]["event"] == "APPROVE" and submits[0]["r"] == "NEW"


def test_load_commits_parses_and_orders(monkeypatch):
    payload = {"commits": [
        {"oid": "aaa111", "messageHeadline": "first", "messageBody": "body one",
         "authoredDate": "2024-01-01T00:00:00Z", "authors": [{"login": "alice"}]},
        {"oid": "bbb222", "messageHeadline": "second", "messageBody": "",
         "authoredDate": "2024-01-02T00:00:00Z", "authors": [{"name": "Bob"}]},
    ]}
    monkeypatch.setattr(api, "gh_json", lambda args: payload)
    commits = api.load_commits(7)
    # newest first: gh returns oldest first, load_commits reverses
    assert [c.oid for c in commits] == ["bbb222", "aaa111"]
    assert commits[0].short == "bbb222"
    assert commits[0].author == "Bob" and commits[1].author == "alice"
    assert commits[0].headline == "second"


def test_load_diff_range_uses_git_range(monkeypatch):
    calls = []

    def fake_sh(args):
        calls.append(args)
        return "diff --git a/f.py b/f.py\n@@ -1 +1 @@\n+x\n"

    monkeypatch.setattr(api, "sh", fake_sh)
    diff, info = api.load_diff_range("aaa", "bbb")
    assert calls == [["git", "diff", f"-U{api.DIFF_CONTEXT}", "aaa^..bbb"]]
    assert "f.py" in diff


def test_load_prs_queries_authored_requested_and_reviewed(monkeypatch):
    searches = []

    def fake_gh_json(args):
        # args like: ["pr", "list", "--limit", "50", "--search", <q>, "--json", ...]
        q = args[args.index("--search") + 1]
        searches.append(q)
        if "author:@me" in q:
            return [{"number": 3, "title": "mine", "headRefName": "b3",
                     "author": {"login": "me"}, "id": "i3"}]
        if "review-requested:@me" in q:
            return [{"number": 2, "title": "req", "headRefName": "b2",
                     "author": {"login": "x"}, "id": "i2"}]
        if "reviewed-by:@me" in q:
            # includes a dup (2) plus a new one (1)
            return [{"number": 2, "title": "req", "headRefName": "b2",
                     "author": {"login": "x"}, "id": "i2"},
                    {"number": 1, "title": "reviewed", "headRefName": "b1",
                     "author": {"login": "y"}, "id": "i1"}]
        return []

    monkeypatch.setattr(api, "gh_json", fake_gh_json)
    prs = api.load_prs()
    assert any("reviewed-by:@me" in s for s in searches)
    # deduped by number, sorted descending
    assert [p.number for p in prs] == [3, 2, 1]


def test_add_pending_range_includes_start_line(monkeypatch):
    from ghreview.models import PendingComment
    found = {"data": {"repository": {"pullRequest": {
        "id": "PR", "reviews": {"nodes": [{"id": "REV"}]}}}}}
    fake = FakeGraphQL(responses={"reviews": found, "addPullRequestReviewThread": {}})
    monkeypatch.setattr(api, "gh_graphql", fake)
    c = PendingComment("a.py", "hi", 10, "RIGHT", start_line=5, start_side="RIGHT")
    api.add_pending_comment_api("o", "n", 1, "me", "PR", c)
    q, v = [(q, v) for q, v in fake.calls if "addPullRequestReviewThread" in q][0]
    assert "startLine:$startLine" in q and "startSide:$startSide" in q
    assert v["startLine"] == 5 and v["startSide"] == "RIGHT"
    assert v["line"] == 10 and v["side"] == "RIGHT"


def test_add_pending_single_omits_start_line(monkeypatch):
    from ghreview.models import PendingComment
    found = {"data": {"repository": {"pullRequest": {
        "id": "PR", "reviews": {"nodes": [{"id": "REV"}]}}}}}
    fake = FakeGraphQL(responses={"reviews": found, "addPullRequestReviewThread": {}})
    monkeypatch.setattr(api, "gh_graphql", fake)
    api.add_pending_comment_api("o", "n", 1, "me", "PR",
                                PendingComment("a.py", "hi", 10, "RIGHT"))
    q, v = [(q, v) for q, v in fake.calls if "addPullRequestReviewThread" in q][0]
    assert "startLine" not in q and "startLine" not in v


def test_load_prs_categorizes_mine_vs_review(monkeypatch):
    def fake_gh_json(args):
        q = args[args.index("--search") + 1]
        if "author:@me" in q:
            return [{"number": 3, "title": "mine", "headRefName": "b3",
                     "author": {"login": "me"}, "id": "i3"}]
        if "review-requested:@me" in q:
            return [{"number": 5, "title": "req", "headRefName": "b5",
                     "author": {"login": "x"}, "id": "i5"}]
        if "reviewed-by:@me" in q:
            return [{"number": 4, "title": "rev", "headRefName": "b4",
                     "author": {"login": "y"}, "id": "i4"}]
        return []

    monkeypatch.setattr(api, "gh_json", fake_gh_json)
    prs = api.load_prs()
    # mine group first (even though #3 < #5), then review newest-first
    assert [(p.number, p.category) for p in prs] == [
        (3, "mine"), (5, "review"), (4, "review")]


def test_update_pending_comment_api(monkeypatch):
    fake = FakeGraphQL()
    monkeypatch.setattr(api, "gh_graphql", fake)
    api.update_pending_comment_api("C1", "new body")
    q, v = fake.calls[0]
    assert "updatePullRequestReviewComment" in q
    assert v == {"id": "C1", "body": "new body"}
