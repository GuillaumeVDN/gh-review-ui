import json

import pytest

from ghreview import gh


def test_gh_graphql_raises_on_errors_payload(monkeypatch):
    resp = json.dumps({
        "data": {"addPullRequestReviewThread": None},
        "errors": [{"message": "pull_request_review_thread.line must be part of the diff"}],
    })
    monkeypatch.setattr(gh, "sh", lambda args: resp)
    with pytest.raises(RuntimeError, match="must be part of the diff"):
        gh.gh_graphql("mutation { ... }")


def test_gh_graphql_returns_data_when_ok(monkeypatch):
    resp = json.dumps({"data": {"viewer": {"login": "me"}}})
    monkeypatch.setattr(gh, "sh", lambda args: resp)
    assert gh.gh_graphql("query { viewer { login } }") == {"data": {"viewer": {"login": "me"}}}


def test_gh_graphql_passes_typed_variables(monkeypatch):
    captured = {}

    def fake_sh(args):
        captured["args"] = args
        return json.dumps({"data": {}})

    monkeypatch.setattr(gh, "sh", fake_sh)
    gh.gh_graphql("q", number=7, flag=True, name="x", skip=None)
    args = captured["args"]
    assert "-F" in args and "number=7" in args         # int → -F
    assert "flag=true" in args                          # bool → -F true
    assert "name=x" in args                             # str → -f
    assert not any("skip" in a for a in args)           # None dropped
