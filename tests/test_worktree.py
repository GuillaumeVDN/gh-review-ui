import os

from ghreview import api, editor
from ghreview.models import State, FileEntry


def test_worktree_path_under_cache(monkeypatch):
    monkeypatch.setenv("XDG_CACHE_HOME", "/tmp/cache")
    p = api.worktree_path("owner", "repo", 42)
    assert p == "/tmp/cache/gh-review-ui/worktrees/owner__repo/pr-42"


def test_base_remote_matches_owner_repo(monkeypatch):
    remote_v = (
        "origin\tgit@github.com:me/fork.git (fetch)\n"
        "origin\tgit@github.com:me/fork.git (push)\n"
        "upstream\tgit@github.com:acme/widgets.git (fetch)\n"
        "upstream\tgit@github.com:acme/widgets.git (push)\n"
    )
    monkeypatch.setattr(api, "sh", lambda args, **kw: remote_v)
    assert api.base_remote("/repo", "acme", "widgets") == "upstream"
    assert api.base_remote("/repo", "me", "fork") == "origin"


def test_base_remote_defaults_to_origin_on_error(monkeypatch):
    def boom(*a, **k):
        raise RuntimeError("no git")
    monkeypatch.setattr(api, "sh", boom)
    assert api.base_remote("/repo", "x", "y") == "origin"


def test_open_pr_worktree_creates_new(monkeypatch, tmp_path):
    calls = []

    def fake_sh(args, **kw):
        calls.append(args)
        if args[:2] == ["git", "-C"] and args[3:4] == ["rev-parse"] and "--is-inside-work-tree" in args:
            raise RuntimeError("not a worktree")  # path doesn't exist yet
        if "rev-parse" in args:
            return "deadbeef\n"
        if "remote" in args:
            return "origin\tgit@github.com:o/n.git (fetch)\n"
        return ""

    monkeypatch.setattr(api, "sh", fake_sh)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path))
    path = api.open_pr_worktree("/repo", "o", "n", 7)

    assert path.endswith("o__n/pr-7")
    joined = [" ".join(c) for c in calls]
    # fetched the PR head ref and created the worktree at the fetched sha
    assert any("fetch origin +refs/pull/7/head:refs/gh-review-ui/pr-7" in j for j in joined)
    assert any("worktree add --force -B gh-review-ui/pr-7" in j and "deadbeef" in j for j in joined)


def test_open_pr_worktree_refreshes_existing(monkeypatch, tmp_path):
    wt = tmp_path / "gh-review-ui" / "worktrees" / "o__n" / "pr-7"
    wt.mkdir(parents=True)
    calls = []

    def fake_sh(args, **kw):
        calls.append(args)
        if "--is-inside-work-tree" in args:
            return "true\n"  # existing valid worktree
        if "rev-parse" in args:
            return "cafef00d\n"
        if "remote" in args:
            return "origin\tgit@github.com:o/n.git (fetch)\n"
        return ""

    monkeypatch.setattr(api, "sh", fake_sh)
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path))
    api.open_pr_worktree("/repo", "o", "n", 7)

    joined = [" ".join(c) for c in calls]
    # existing worktree is reset to the fresh sha via checkout -B (no new add)
    assert any("checkout -B gh-review-ui/pr-7 cafef00d" in j for j in joined)
    assert not any("worktree add" in j for j in joined)


def test_editor_uses_active_worktree(monkeypatch):
    opened = {}
    monkeypatch.setattr(editor, "open_in_editor",
                        lambda path, line: opened.update(path=path, line=line))
    st = State()
    st.repo_root = "/main/repo"
    st.active_worktree = "/cache/pr-7"
    st.files = [FileEntry("src/a.py", False)]
    st.tree = [(0, "a.py", "file", 0, None)]
    st.file_idx = 0
    editor.open_current_in_editor(st, top=True)
    assert opened["path"] == os.path.join("/cache/pr-7", "src/a.py")
    assert opened["line"] == 1


def test_editor_falls_back_to_repo_root(monkeypatch):
    opened = {}
    monkeypatch.setattr(editor, "open_in_editor",
                        lambda path, line: opened.update(path=path))
    st = State()
    st.repo_root = "/main/repo"
    st.active_worktree = ""
    st.files = [FileEntry("a.py", False)]
    st.tree = [(0, "a.py", "file", 0, None)]
    st.file_idx = 0
    editor.open_current_in_editor(st, top=True)
    assert opened["path"] == os.path.join("/main/repo", "a.py")


def test_last_pr_roundtrip(monkeypatch, tmp_path):
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path))
    assert api.load_last_pr("o", "n") is None
    api.save_last_pr("o", "n", 42)
    api.save_last_pr("o", "other", 7)
    assert api.load_last_pr("o", "n") == 42
    assert api.load_last_pr("o", "other") == 7
    # updating overwrites
    api.save_last_pr("o", "n", 99)
    assert api.load_last_pr("o", "n") == 99


def test_last_pr_missing_file(monkeypatch, tmp_path):
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "nope"))
    assert api.load_last_pr("o", "n") is None
