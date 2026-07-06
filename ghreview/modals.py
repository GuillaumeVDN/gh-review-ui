"""Blocking modal loops: the comment editor and the finish-review dialog."""
import curses

from . import theme
from .keys import get_key
from .render import safe_addstr, draw_box
from .textbuffer import TextArea, wrap_textarea

REVIEW_CHOICES = [
    ("COMMENT", "Comment"),
    ("REQUEST_CHANGES", "Request changes"),
    ("APPROVE", "Approve"),
]


def curs_set(v):
    try:
        curses.curs_set(v)
    except curses.error:
        pass


def _blank_interior(stdscr, my, mx, mh, mw):
    for r in range(1, mh - 1):
        safe_addstr(stdscr, my + r, mx + 1, " " * (mw - 2))


def show_editor_modal(stdscr, title, help_line, initial=""):
    """Multi-line editor with soft-wrap. Returns (action, text).

    action ∈ {'enter', 'ctrl_enter', 'cancel'}.
    """
    ta = TextArea(initial)
    curs_set(1)
    try:
        while True:
            H, W = stdscr.getmaxyx()
            mh = max(8, min(20, H - 4))
            mw = max(40, min(90, W - 4))
            my, mx = (H - mh) // 2, (W - mw) // 2
            _blank_interior(stdscr, my, mx, mh, mw)
            draw_box(stdscr, my, mx, mh, mw, title, True)
            safe_addstr(stdscr, my + mh - 2, mx + 2, help_line, theme.style("keys", dim=True), maxw=mw - 4)
            editor_h, editor_w = mh - 3, mw - 4
            # Soft-wrap for display only — no real newline enters the text.
            visual, cur_vrow, cur_vcol = wrap_textarea(ta, editor_w)
            first = max(0, cur_vrow - editor_h + 1)
            for i in range(editor_h):
                idx = first + i
                if idx >= len(visual):
                    break
                safe_addstr(stdscr, my + 1 + i, mx + 2, visual[idx][2], 0, maxw=editor_w)
            try:
                stdscr.move(my + 1 + (cur_vrow - first), mx + 2 + cur_vcol)
            except curses.error:
                pass
            stdscr.refresh()
            ch = get_key(stdscr)
            if ch == -1:
                continue
            action = ta.handle(ch)
            if action:
                return action, ta.text()
    finally:
        curs_set(0)


def show_review_modal(stdscr, n_pending, initial=""):
    """Finish-review dialog: description editor (top) + event choice (bottom).

    Enter in the editor moves focus to the choices; Enter on a choice submits.
    Returns (event, body) or None if cancelled.
    """
    ta = TextArea(initial)
    sel = 0
    editing = True
    try:
        while True:
            H, W = stdscr.getmaxyx()
            mh = max(12, min(24, H - 4))
            mw = max(48, min(90, W - 4))
            my, mx = (H - mh) // 2, (W - mw) // 2
            _blank_interior(stdscr, my, mx, mh, mw)
            title = f"Finish review · {n_pending} pending comment{'s' if n_pending != 1 else ''}"
            draw_box(stdscr, my, mx, mh, mw, title, True)
            inner_w = mw - 4
            editor_h = mh - 2 - len(REVIEW_CHOICES) - 2
            first = max(0, ta.row - editor_h + 1)
            for i in range(editor_h):
                idx = first + i
                txt = ta.lines[idx].expandtabs(4) if idx < len(ta.lines) else ""
                safe_addstr(stdscr, my + 1 + i, mx + 2, txt, 0, maxw=inner_w)
            div_y = my + 1 + editor_h
            safe_addstr(stdscr, div_y, mx + 1, "─" * (mw - 2), curses.A_DIM)
            for i, (_ev, label) in enumerate(REVIEW_CHOICES):
                focused_choice = (not editing) and i == sel
                if focused_choice:
                    a = theme.style("sel", bold=True)
                elif not editing:
                    a = theme.style("focus", bold=True)
                else:
                    a = curses.A_DIM
                marker = "▸ " if focused_choice else "  "
                safe_addstr(stdscr, div_y + 1 + i, mx + 2, (marker + label).ljust(inner_w), a, maxw=inner_w)
            help_line = ("Shift+Enter: newline · Enter: choose event · Esc: cancel"
                         if editing else
                         "j/k: select · Enter: submit · k at top: back · Esc: cancel")
            safe_addstr(stdscr, my + mh - 2, mx + 2, help_line, theme.style("keys", dim=True), maxw=mw - 4)
            if editing:
                curs_set(1)
                try:
                    stdscr.move(my + 1 + (ta.row - first), mx + 2 + min(ta.col, inner_w - 1))
                except curses.error:
                    pass
            else:
                curs_set(0)
            stdscr.refresh()
            ch = get_key(stdscr)
            if ch == -1:
                continue
            if editing:
                action = ta.handle(ch)
                if action == "cancel":
                    return None
                if action in ("enter", "ctrl_enter"):
                    editing = False
            else:
                if ch == 27:
                    return None
                elif ch in (curses.KEY_UP, ord("k")):
                    if sel == 0:
                        editing = True
                    else:
                        sel -= 1
                elif ch in (curses.KEY_DOWN, ord("j")):
                    sel = min(len(REVIEW_CHOICES) - 1, sel + 1)
                elif ch in (curses.KEY_ENTER, 10, 13):
                    return REVIEW_CHOICES[sel][0], ta.text()
    finally:
        curs_set(0)
