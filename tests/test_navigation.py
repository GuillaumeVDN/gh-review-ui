from ghreview.models import State, FileEntry
from ghreview.navigation import (
    cur_file_path, current_hunk_range, hunk_anchor_line, jump_hunk,
    current_hunk_editor_line, hunk_for_comment, scroll_diff,
)
from ghreview.models import PendingComment


def diff_state():
    st = State()
    st.files = [FileEntry("f.py", False)]
    st.tree = [(0, "f.py", "file", 0, None)]
    st.file_idx = 0
    lines = [
        "@@ -1,2 +1,3 @@",   # 0
        " ctx",             # 1
        "+added",           # 2
        "+added2",          # 3
        "@@ -10,1 +11,2 @@", # 4
        " keep",            # 5
        "-removed",         # 6
    ]
    info = [
        (None, None),
        (1, 1),
        (None, 2),
        (None, 3),
        (None, None),
        (11, 11),
        (12, None),
    ]
    st.diff_by_file = {"f.py": lines}
    st.info_by_file = {"f.py": info}
    st.hunks_by_file = {"f.py": [(0, 4), (4, 7)]}
    return st


def test_cur_file_path_none_for_dir():
    st = State()
    st.files = [FileEntry("a.py", False)]
    st.tree = [(0, "src", "dir", "src", False)]
    st.file_idx = 0
    assert cur_file_path(st) is None


def test_current_hunk_tracks_index_not_scroll():
    st = diff_state()
    st.diff_hunk_idx = 0
    assert current_hunk_range(st, "f.py") == (0, 4)
    st.diff_hunk_idx = 1
    st.diff_scroll = 0  # scroll clamped to 0, highlight must still follow index
    assert current_hunk_range(st, "f.py") == (4, 7)


def test_jump_hunk_advances_and_clamps():
    st = diff_state()
    assert st.diff_hunk_idx == 0
    jump_hunk(st, +1)
    assert st.diff_hunk_idx == 1 and st.diff_scroll == 4
    jump_hunk(st, +1)  # clamp at last
    assert st.diff_hunk_idx == 1
    jump_hunk(st, -1)
    assert st.diff_hunk_idx == 0 and st.diff_scroll == 0


def test_hunk_anchor_first_commentable_line_right_side():
    st = diff_state()
    st.diff_hunk_idx = 0
    # first non-header line is the context line (new-side line 1)
    assert hunk_anchor_line(st, "f.py") == (1, "RIGHT")


def test_editor_line_is_first_added():
    st = diff_state()
    st.diff_hunk_idx = 0
    assert current_hunk_editor_line(st, "f.py") == 2  # +added


def test_editor_line_pure_deletion_falls_back():
    st = diff_state()
    st.diff_hunk_idx = 1  # second hunk: keep(new 11), removed(old only)
    assert current_hunk_editor_line(st, "f.py") == 11


def test_hunk_for_comment_locates_target():
    st = diff_state()
    c = PendingComment(path="f.py", body="x", line=3, side="RIGHT")
    hunk_lines, target = hunk_for_comment(st, c)
    assert hunk_lines == st.diff_by_file["f.py"][0:4]
    assert hunk_lines[target] == "+added2"


def test_scroll_diff_floor_zero():
    st = State()
    st.diff_scroll = 1
    scroll_diff(st, -5)
    assert st.diff_scroll == 0
