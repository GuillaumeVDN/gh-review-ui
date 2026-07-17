import curses

from ghreview import theme


def test_classify_diff_line():
    assert theme.classify_diff_line("+added") == "add"
    assert theme.classify_diff_line("+++ b/f") == "meta"
    assert theme.classify_diff_line("-gone") == "del"
    assert theme.classify_diff_line("--- a/f") == "meta"
    assert theme.classify_diff_line("@@ -1 +1 @@") == "hunk"
    assert theme.classify_diff_line("diff --git a/f b/f") == "meta"
    assert theme.classify_diff_line(" context") == "context"


def test_style_flags_without_color_init():
    # No curses screen: color names contribute nothing, flags still apply.
    assert theme.style("nosuch") == 0
    assert theme.style(bold=True) == curses.A_BOLD
    assert theme.style(dim=True) == curses.A_DIM
    assert theme.style("x", bold=True, underline=True) == (curses.A_BOLD | curses.A_UNDERLINE)


def test_diff_line_style_meta_is_bold():
    # meta lines are bold even before color pairs are allocated.
    assert theme.diff_line_style("diff --git a/f b/f") == curses.A_BOLD
    assert theme.diff_line_style("@@ x @@") == curses.A_BOLD  # hunk header bold


def test_current_hunk_highlights_only_changed_lines(monkeypatch):
    # Route style() to a readable sentinel so we can see which pair is chosen.
    monkeypatch.setattr(theme, "hl_enabled", True)
    monkeypatch.setattr(theme, "style", lambda name="", **k: f"S:{name}:{sorted(k)}")
    # changed lines in the focused hunk get the highlight-background pairs
    assert theme.diff_line_style("+added", current=True) == "S:hl.add:[]"
    assert theme.diff_line_style("-gone", current=True) == "S:hl.del:[]"
    # context and header do NOT get a hl.* band while current
    assert theme.diff_line_style(" ctx", current=True) == 0
    assert theme.diff_line_style("@@ h @@", current=True) == "S:hunk:['bold']"
    # outside the focused hunk, changed lines use their normal (non-hl) style
    assert theme.diff_line_style("+added", current=False) == "S:add:[]"


def test_detail_styles_has_expected_kinds():
    styles = theme.detail_styles()
    for kind in ("title", "meta", "sep", "h1", "h2", "h3", "summary",
                 "bullet", "quote", "code", "rule", "dim"):
        assert kind in styles
