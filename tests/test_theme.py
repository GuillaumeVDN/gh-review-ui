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


def test_detail_styles_has_expected_kinds():
    styles = theme.detail_styles()
    for kind in ("title", "meta", "sep", "h1", "h2", "h3", "summary",
                 "bullet", "quote", "code", "rule", "dim"):
        assert kind in styles
