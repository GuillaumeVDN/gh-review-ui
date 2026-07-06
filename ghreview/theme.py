"""Colors and generic highlight helpers.

Color pairs are registered by *semantic name* so the rest of the UI asks for
``style("add")`` rather than juggling raw pair numbers. ``init`` must run once
after curses starts; the pure ``classify_diff_line`` needs no curses and is
unit-tested directly.
"""
import curses

HL_BG_COLOR = 23  # muted cyan — same hue as the @@ headers, but lighter

# Base pairs: (name, fg, bg). bg == -1 means the terminal default.
_BASE = [
    ("add",     curses.COLOR_GREEN, -1),
    ("del",     curses.COLOR_RED, -1),
    ("hunk",    curses.COLOR_CYAN, -1),
    ("focus",   curses.COLOR_GREEN, -1),
    ("active",  curses.COLOR_CYAN, -1),
    ("sel",     curses.COLOR_BLACK, curses.COLOR_GREEN),
    ("status",  curses.COLOR_YELLOW, -1),
    ("keys",    curses.COLOR_WHITE, -1),
    ("title",   curses.COLOR_YELLOW, -1),
    ("curhunk", curses.COLOR_GREEN, -1),
]
# Highlighted-hunk background variants (need 256 colors).
_HL = [
    ("hl.add", curses.COLOR_GREEN, HL_BG_COLOR),
    ("hl.del", curses.COLOR_RED, HL_BG_COLOR),
    ("hl.ctx", -1, HL_BG_COLOR),
    ("hl.hdr", curses.COLOR_WHITE, HL_BG_COLOR),
]

_pairs = {}          # name -> pair index
hl_enabled = False   # True when the background-highlight pairs are available


def init():
    """Allocate all color pairs. Call once, after curses is initialised."""
    global hl_enabled
    _pairs.clear()
    curses.use_default_colors()
    idx = 1
    for name, fg, bg in _BASE:
        curses.init_pair(idx, fg, bg)
        _pairs[name] = idx
        idx += 1
    hl_enabled = False
    try:
        if curses.COLORS >= 256:
            for name, fg, bg in _HL:
                curses.init_pair(idx, fg, bg)
                _pairs[name] = idx
                idx += 1
            hl_enabled = True
    except (curses.error, ValueError):
        hl_enabled = False


def style(name="", *, bold=False, dim=False, reverse=False, underline=False):
    """Return a curses attribute for a named pair plus optional flags.

    An unknown/empty name contributes no color (just the flags), so
    ``style(bold=True)`` is a plain bold attribute.
    """
    attr = curses.color_pair(_pairs[name]) if name in _pairs else 0
    if bold:
        attr |= curses.A_BOLD
    if dim:
        attr |= curses.A_DIM
    if reverse:
        attr |= curses.A_REVERSE
    if underline:
        attr |= curses.A_UNDERLINE
    return attr


def classify_diff_line(line):
    """Classify a diff line: add / del / hunk / meta / context (pure)."""
    if line.startswith("+") and not line.startswith("+++"):
        return "add"
    if line.startswith("-") and not line.startswith("---"):
        return "del"
    if line.startswith("@@"):
        return "hunk"
    if line.startswith(("diff --git", "index ", "+++", "---")):
        return "meta"
    return "context"


def diff_line_style(line, current=False):
    """Attribute for a diff line, honoring the current-hunk highlight band."""
    kind = classify_diff_line(line)
    if current and hl_enabled:
        return {
            "add": style("hl.add"),
            "del": style("hl.del"),
            "hunk": style("hl.hdr", bold=True),
        }.get(kind, style("hl.ctx"))
    return {
        "add": style("add"),
        "del": style("del"),
        "hunk": style("hunk", bold=True),
        "meta": style(bold=True),
    }.get(kind, 0)


def hunk_marker_style():
    """Attribute for the green side-bar marking the current hunk."""
    return style("hl.add" if hl_enabled else "curhunk", bold=True)


# Style map for markdown "kind" hints used by the PR summary pane.
def detail_styles():
    return {
        "title": style("focus", bold=True),
        "meta": style("active"),
        "sep": style("hunk", bold=True),
        "h1": style("title", bold=True, underline=True),
        "h2": style("title", bold=True),
        "h3": style(bold=True),
        "summary": style("active", bold=True),
        "bullet": 0,
        "quote": style(dim=True),
        "code": style("hunk"),
        "rule": style(dim=True),
        "dim": style(dim=True),
    }
