from ghreview.render import clamp_view, compute_layout, shortcuts_for
from ghreview.models import State, FOCUS_PRS, FOCUS_PENDING, FOCUS_FILES, FOCUS_DIFF


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
    assert set(rects) == {"prs", "pending", "files", "right"}
    py, px, ph, pw = rects["prs"]
    fy, fx, fh, fw = rects["files"]
    ry, rx, rh, rw = rects["right"]
    # left column stacks to body height; right pane fills the rest of the width
    assert px == fx == 0
    assert rx == pw
    assert pw + rw == 120
    # left panes are stacked contiguously
    assert rects["pending"][0] == py + ph
    assert fy == rects["pending"][0] + rects["pending"][2]


def test_shortcuts_change_with_focus():
    st = State()
    for focus, needle in [(FOCUS_PRS, "checkout"), (FOCUS_PENDING, "submit review"),
                          (FOCUS_FILES, "fold viewed"), (FOCUS_DIFF, "comment")]:
        st.focus = focus
        assert needle in shortcuts_for(st)
