import queue

from ghreview.models import State, PR, Commit, FileEntry, PendingComment
from ghreview import controller


def test_submit_job_marks_busy_and_enqueues():
    st = State()
    jobs = queue.Queue()
    controller.submit_job(jobs, st, ("load_prs",))
    assert "prs" in st.busy
    assert jobs.get_nowait() == ("load_prs",)


def test_apply_prs_result():
    st = State()
    st.busy.add("prs")
    controller.apply_result(st, ("prs", [PR(1, "t", "h", "a")]), queue.Queue())
    assert len(st.prs) == 1 and "prs" not in st.busy


def test_apply_active_populates_diff_and_tree():
    st = State()
    st.busy.add("active")
    lines = ["diff --git a/f.py b/f.py", "@@ -1 +1 @@", "+x"]
    diff = {"f.py": lines}
    info = {"f.py": [(None, None), (None, None), (None, 1)]}
    pending = [PendingComment("f.py", "hi", 1, "RIGHT", "C1")]
    commits = [Commit("abc123", "first")]
    controller.apply_result(st, ("active", 7, "PRID", [FileEntry("f.py", False)],
                                 diff, info, pending, commits), queue.Queue())
    assert st.active_pr.number == 7
    assert st.hunks_by_file["f.py"] == [(1, 3)]
    assert st.tree and st.pending == pending
    assert st.commits == commits and st.commit_selected == {"abc123"}
    assert "active" not in st.busy


def test_apply_commit_selection_fills_range_and_submits():
    st = State()
    st.active_pr = PR(1, "t", "h", "a")
    # newest first: c0 newest .. c4 oldest
    st.commits = [Commit(f"c{i}", f"m{i}") for i in range(5)]
    st.commit_selected = {"c1", "c3"}  # non-contiguous
    jobs = queue.Queue()
    controller.apply_commit_selection(st, jobs)
    # gap filled to a contiguous range c1..c3
    assert st.commit_selected == {"c1", "c2", "c3"}
    # diff runs from parent of the oldest (c3) through the newest (c1)
    assert jobs.get_nowait() == ("load_commit_diff", "c3", "c1")
    assert "commitdiff" in st.busy


def test_apply_commit_selection_empty_warns():
    st = State()
    st.active_pr = PR(1, "t", "h", "a")
    st.commits = [Commit("c0", "m0")]
    st.commit_selected = set()
    jobs = queue.Queue()
    controller.apply_commit_selection(st, jobs)
    assert jobs.empty() and "at least one" in st.status


def test_apply_commit_diff_filters_files_preserving_viewed():
    st = State()
    st.busy.add("commitdiff")
    st.viewed_by_path = {"a.py": True, "b.py": False}
    lines = ["diff --git a/a.py b/a.py", "@@ -1 +1 @@", "+x"]
    diff = {"a.py": lines}
    info = {"a.py": [(None, None), (None, None), (None, 1)]}
    controller.apply_result(st, ("commit_diff", diff, info), queue.Queue())
    assert [f.path for f in st.files] == ["a.py"]
    assert st.files[0].viewed is True
    assert st.hunks_by_file["a.py"] == [(1, 3)]
    assert "commitdiff" not in st.busy


def test_apply_viewed_ok_updates_files():
    st = State()
    st.files = [FileEntry("a.py", False), FileEntry("b.py", False)]
    st.busy.add("viewed")
    controller.apply_result(st, ("viewed_ok", ["a.py"], True), queue.Queue())
    assert st.files[0].viewed and not st.files[1].viewed
    assert "viewed" not in st.busy


def test_apply_viewed_bulk_updates_only_done():
    st = State()
    st.files = [FileEntry("a.py", False), FileEntry("b.py", False)]
    st.busy.add("viewed")
    controller.apply_result(st, ("viewed_bulk", ["a.py"], True, [("b.py", "err")]), queue.Queue())
    assert st.files[0].viewed and not st.files[1].viewed
    assert "failed" in st.status


def test_apply_pending_list_replaces():
    st = State()
    st.busy.add("pending")
    st.pending = [PendingComment("a", "x", 1, "RIGHT")]
    controller.apply_result(st, ("pending_list", [], "done"), queue.Queue())
    assert st.pending == [] and st.status == "done"


def test_apply_review_submitted_clears_pending():
    st = State()
    st.pending = [PendingComment("a", "x", 1, "RIGHT")]
    st.busy.add("review")
    controller.apply_result(st, ("review_submitted", "APPROVE"), queue.Queue())
    assert st.pending == [] and "review" not in st.busy
    assert "APPROVE" in st.status


def test_apply_error_clears_busy_and_reports():
    st = State()
    st.busy.add("pending")
    controller.apply_result(st, ("error", "add_pending", "kaboom"), queue.Queue())
    assert "pending" not in st.busy
    assert "kaboom" in st.status


def test_pr_opened_sets_worktree_and_triggers_load():
    st = State()
    st.repo_owner, st.repo_name, st.viewer = "o", "n", "me"
    st.busy.add("worktree")
    jobs = queue.Queue()
    controller.apply_result(st, ("pr_opened", 5, "/cache/pr-5"), jobs)
    assert st.active_worktree == "/cache/pr-5"
    assert "worktree" not in st.busy
    # load_active is submitted for that specific PR number
    assert jobs.get_nowait() == ("load_active", "o", "n", "me", 5)


def test_apply_active_none_clears_worktree():
    st = State()
    st.active_worktree = "/cache/pr-9"
    st.busy.add("active")
    controller.apply_result(st, ("active", None, None, [], {}, {}, [], []), queue.Queue())
    assert st.active_pr is None and st.active_worktree == ""


def test_pane_at_hit_testing():
    st = State()
    st.rects = {"prs": (0, 0, 5, 10), "right": (0, 10, 20, 30)}
    assert controller.pane_at(st, 2, 3) == "prs"
    assert controller.pane_at(st, 1, 15) == "right"
    assert controller.pane_at(st, 50, 50) is None
