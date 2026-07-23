from ghreview.render import clamp_view, compute_layout, shortcuts_for
from ghreview.models import (State, FOCUS_PRS, FOCUS_COMMITS, FOCUS_PENDING,
                             FOCUS_FILES, FOCUS_DIFF)


def test_clamp_view_keeps_selection_visible():
    # selection above the window -> scroll up to it
    assert clamp_view(0, 5, 10, 100) == 0
    # selection below the window -> scroll so it's the last row
    assert clamp_view(20, 0, 10, 100) == 11
    # selection already visible -> unchanged
    assert clamp_view(3, 0, 10, 100) == 0
    # empty list
    assert clamp_view(0, 0, 10, 0) == 0


def test_clamp_view_does_not_overscroll_past_end():
    # offset beyond the last full page is pulled back
    assert clamp_view(95, 90, 10, 100) == 90


def test_compute_layout_covers_screen_without_overlap():
    rects = compute_layout(40, 120)
    assert set(rects) == {"prs", "commits", "pending", "files", "right"}
    py, px, ph, pw = rects["prs"]
    fy, fx, fh, fw = rects["files"]
    ry, rx, rh, rw = rects["right"]
    # left column stacks to body height; right pane fills the rest of the width
    assert px == fx == 0
    assert rx == pw
    assert pw + rw == 120
    # left panes are stacked contiguously: prs -> commits -> files -> pending
    cy, _, ch, _ = rects["commits"]
    pey, _, peh, _ = rects["pending"]
    assert cy == py + ph
    assert fy == cy + ch
    assert pey == fy + fh
    # they fill exactly the body height
    assert pey + peh == py + (40 - 2)


def test_shortcuts_change_with_focus():
    st = State()
    for focus, needle in [(FOCUS_PRS, "worktree"), (FOCUS_COMMITS, "toggle"),
                          (FOCUS_PENDING, "submit review"),
                          (FOCUS_FILES, "fold viewed"), (FOCUS_DIFF, "comment")]:
        st.focus = focus
        assert needle in shortcuts_for(st)


def test_pr_rows_groups_with_headers():
    from ghreview.render import pr_rows
    from ghreview.models import PR
    prs = [PR(3, "a", "h", "me", category="mine"),
           PR(5, "b", "h", "x", category="review"),
           PR(4, "c", "h", "y", category="review")]
    rows = pr_rows(prs)
    assert rows == [("hdr", "My PRs"), ("pr", 0),
                    ("hdr", "Requested review"), ("pr", 1), ("pr", 2)]


def test_pr_rows_single_group_no_extra_header():
    from ghreview.render import pr_rows
    from ghreview.models import PR
    prs = [PR(1, "a", "h", "me", category="mine"),
           PR(2, "b", "h", "me", category="mine")]
    rows = pr_rows(prs)
    assert rows == [("hdr", "My PRs"), ("pr", 0), ("pr", 1)]


def test_pr_rows_empty():
    from ghreview.render import pr_rows
    assert pr_rows([]) == []


def test_reveal_scroll_no_change_when_visible():
    from ghreview.render import reveal_scroll
    # block [10,13) fully within [8, 8+10): stay put
    assert reveal_scroll(8, 10, 13, 10) == 8


def test_reveal_scroll_up_when_above():
    from ghreview.render import reveal_scroll
    assert reveal_scroll(20, 5, 8, 10) == 5   # target above viewport → scroll to it


def test_reveal_scroll_down_minimal_when_below():
    from ghreview.render import reveal_scroll
    # block [15,18) below [0,10): scroll so its end (18) is the last row → 8
    assert reveal_scroll(0, 15, 18, 10) == 8


def test_reveal_scroll_tall_block_aligns_top():
    from ghreview.render import reveal_scroll
    # block taller than the viewport → align to its top
    assert reveal_scroll(0, 5, 40, 10) == 5
