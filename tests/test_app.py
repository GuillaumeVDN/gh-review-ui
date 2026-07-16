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
