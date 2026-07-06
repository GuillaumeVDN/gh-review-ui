import queue

from ghreview.models import State, PR, FileEntry, PendingComment
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
    controller.apply_result(st, ("active", 7, "PRID", [FileEntry("f.py", False)],
                                 diff, info, pending), queue.Queue())
    assert st.active_pr.number == 7
    assert st.hunks_by_file["f.py"] == [(1, 3)]
    assert st.tree and st.pending == pending
    assert "active" not in st.busy


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


def test_checkout_done_triggers_reload():
    st = State()
    st.repo_owner, st.repo_name, st.viewer = "o", "n", "me"
    st.busy.add("checkout")
    jobs = queue.Queue()
    controller.apply_result(st, ("checkout_done", 5), jobs)
    assert jobs.get_nowait() == ("load_active", "o", "n", "me")


def test_pane_at_hit_testing():
    st = State()
    st.rects = {"prs": (0, 0, 5, 10), "right": (0, 10, 20, 30)}
    assert controller.pane_at(st, 2, 3) == "prs"
    assert controller.pane_at(st, 1, 15) == "right"
    assert controller.pane_at(st, 50, 50) is None
