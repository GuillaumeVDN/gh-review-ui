from ghreview.models import State, FileEntry
from ghreview.navigation import (
    cur_file_path, current_hunk_range, jump_hunk, current_hunk_editor_line,
    hunk_for_comment, scroll_diff, line_target, hunk_line_indices,
    first_change_index,
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
    st.diff_scroll = 7
    assert st.diff_hunk_idx == 0
    jump_hunk(st, +1)
    assert st.diff_hunk_idx == 1
    jump_hunk(st, +1)  # clamp at last
    assert st.diff_hunk_idx == 1
    jump_hunk(st, -1)
    assert st.diff_hunk_idx == 0
    # jump_hunk only moves the selection; scrolling is a render-time concern
    assert st.diff_scroll == 7


def test_line_target_by_kind():
    st = diff_state()
    # info rows: 1=ctx(1,1) 2=+added(_,2) 3=+added2(_,3) 5=keep(11,11) 6=-removed(12,_)
    assert line_target(st, "f.py", 2) == (2, "RIGHT")   # added
    assert line_target(st, "f.py", 6) == (12, "LEFT")   # deleted
    assert line_target(st, "f.py", 1) == (1, "RIGHT")   # context → new side
    assert line_target(st, "f.py", 0) is None           # @@ header row


def test_hunk_line_indices_excludes_header():
    st = diff_state()
    st.diff_hunk_idx = 0                # hunk (0,4): header + ctx + 2 added
    assert hunk_line_indices(st, "f.py") == [1, 2, 3]


def test_first_change_index_prefers_changed_line():
    st = diff_state()
    st.diff_hunk_idx = 0
    assert first_change_index(st, "f.py") == 2   # first +added, not the context
    st.diff_hunk_idx = 1
    assert first_change_index(st, "f.py") == 6   # the deletion


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


def test_jump_file_skips_folders():
    from ghreview.app import _jump_file
    st = State()
    # tree: dir, file, dir, file, file
    st.tree = [
        (0, "src", "dir", "src", False),
        (1, "a.py", "file", 0, None),
        (0, "lib", "dir", "lib", False),
        (1, "b.py", "file", 1, None),
        (1, "c.py", "file", 2, None),
    ]
    st.file_idx = 1  # on a.py
    _jump_file(st, +1)
    assert st.file_idx == 3  # jumped over the "lib" dir to b.py
    _jump_file(st, +1)
    assert st.file_idx == 4  # c.py
    _jump_file(st, +1)
    assert st.file_idx == 4  # no file below -> stays
    _jump_file(st, -1)
    assert st.file_idx == 3  # back to b.py
    _jump_file(st, -1)
    assert st.file_idx == 1  # skips "lib" and "src", lands on a.py


def test_blocks_are_two_navigable_units():
    from ghreview.diff import parse_diff, compute_hunks
    raw = ("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,4 @@\n"
           "-test\n+test2\n context\n-test3\n+test4\n")
    lines, info = parse_diff(raw)
    st = State()
    st.files = [FileEntry("f", False)]
    st.tree = [(0, "f", "file", 0, None)]
    st.file_idx = 0
    st.diff_by_file = lines
    st.info_by_file = info
    st.hunks_by_file = {"f": compute_hunks(lines["f"])}
    st.diff_hunk_idx = 0

    assert len(st.hunks_by_file["f"]) == 2
    # first block: only its two changed lines are commentable (no context)
    idx0 = hunk_line_indices(st, "f")
    assert [lines["f"][i] for i in idx0] == ["-test", "+test2"]
    # j moves to the second block
    jump_hunk(st, +1)
    idx1 = hunk_line_indices(st, "f")
    assert [lines["f"][i] for i in idx1] == ["-test3", "+test4"]
