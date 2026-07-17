import queue

from ghreview import app
from ghreview.models import (State, PR, FileEntry, FOCUS_PRS, FOCUS_COMMITS,
                             FOCUS_FILES, FOCUS_PENDING, FOCUS_DIFF)


def test_digit_map_after_inversion():
    # Files is [3], Pending is [4] (inverted from before).
    assert app.FOCUS_BY_DIGIT[ord("1")] == FOCUS_PRS
    assert app.FOCUS_BY_DIGIT[ord("2")] == FOCUS_COMMITS
    assert app.FOCUS_BY_DIGIT[ord("3")] == FOCUS_FILES
    assert app.FOCUS_BY_DIGIT[ord("4")] == FOCUS_PENDING
    assert app.FOCUS_BY_DIGIT[ord("0")] == FOCUS_DIFF


def test_tab_order_puts_files_before_pending():
    # Tab cycles by integer focus value; Files must come before Pending.
    assert FOCUS_FILES < FOCUS_PENDING
    assert (FOCUS_COMMITS + 1) == FOCUS_FILES
    assert (FOCUS_FILES + 1) == FOCUS_PENDING


def test_space_marks_viewed_in_files_pane():
    st = State()
    st.focus = FOCUS_FILES
    st.active_pr = PR(1, "t", "h", "a", node_id="PRID")
    st.files = [FileEntry("a.py", False)]
    st.tree = [(0, "a.py", "file", 0, None)]
    st.file_idx = 0
    jobs = queue.Queue()
    keep = app._handle_key(None, st, jobs, ord(" "))
    assert keep is True
    job = jobs.get_nowait()
    assert job[0] == "mark_viewed" and job[2] == "a.py" and job[3] is True


def test_v_no_longer_marks_viewed():
    st = State()
    st.focus = FOCUS_FILES
    st.active_pr = PR(1, "t", "h", "a", node_id="PRID")
    st.files = [FileEntry("a.py", False)]
    st.tree = [(0, "a.py", "file", 0, None)]
    st.file_idx = 0
    jobs = queue.Queue()
    app._handle_key(None, st, jobs, ord("v"))
    assert jobs.empty()  # 'v' is now unbound


def _diff_state():
    from ghreview.models import PR
    st = State()
    st.active_pr = PR(1, "t", "h", "a", node_id="PR")
    st.files = [FileEntry("f.py", False)]
    st.tree = [(0, "f.py", "file", 0, None)]
    st.file_idx = 0
    st.diff_by_file = {"f.py": ["@@ -1,2 +1,3 @@", " ctx", "+a", "+b"]}
    st.info_by_file = {"f.py": [(None, None), (1, 1), (None, 2), (None, 3)]}
    st.hunks_by_file = {"f.py": [(0, 4)]}
    st.diff_hunk_idx = 0
    return st


def test_enter_comment_mode_selects_first_change():
    st = _diff_state()
    app._enter_comment_mode(st)
    assert st.comment_mode is True
    assert st.comment_line == 2       # first added line (not the context row)
    assert st.comment_start is None


def test_comment_move_single_line():
    st = _diff_state()
    app._enter_comment_mode(st)
    app._move_comment(st, "f.py", +1, extend=False)
    assert st.comment_line == 3 and st.comment_start is None
    app._move_comment(st, "f.py", -1, extend=False)
    assert st.comment_line == 2


def test_comment_move_range_with_shift():
    st = _diff_state()
    app._enter_comment_mode(st)         # cursor at 2
    app._move_comment(st, "f.py", +1, extend=True)
    assert st.comment_start == 2 and st.comment_line == 3   # anchored range 2..3
    app._move_comment(st, "f.py", +1, extend=True)          # clamp at end
    assert st.comment_start == 2 and st.comment_line == 3


def test_comment_mode_captures_keys_and_esc_exits():
    import queue
    st = _diff_state()
    app._enter_comment_mode(st)
    # 'q' does not quit while in comment mode
    assert app._handle_key(None, st, queue.Queue(), ord("q")) is True
    assert st.comment_mode is True
    # Esc leaves comment mode
    app._handle_key(None, st, queue.Queue(), 27)
    assert st.comment_mode is False


def test_comment_mode_enter_opens_modal(monkeypatch):
    import queue
    called = []
    monkeypatch.setattr(app, "open_comment_modal",
                        lambda s, st, j: called.append(True))
    st = _diff_state()
    app._enter_comment_mode(st)
    app._handle_key(None, st, queue.Queue(), 10)
    assert called == [True]
