"""The modal text editor buffer and its soft-wrap helper."""
import curses

from .keys import KEY_SHIFT_ENTER, KEY_CTRL_ENTER


class TextArea:
    """A tiny multi-line editor buffer.

    ``handle`` returns an action string for keys the caller must act on
    (``"enter"``, ``"ctrl_enter"``, ``"cancel"``) and None for edits it applied.
    Shift+Enter inserts a real newline; plain Enter is a confirm signal.
    """

    def __init__(self, initial=""):
        self.lines = initial.split("\n") if initial else [""]
        if not self.lines:
            self.lines = [""]
        self.row = len(self.lines) - 1
        self.col = len(self.lines[-1])

    def text(self):
        return "\n".join(self.lines)

    def newline(self):
        cur = self.lines[self.row]
        self.lines[self.row] = cur[:self.col]
        self.lines.insert(self.row + 1, cur[self.col:])
        self.row += 1
        self.col = 0

    def handle(self, ch):
        if ch == 27:
            return "cancel"
        if ch == KEY_SHIFT_ENTER:                     # Shift+Enter → newline
            self.newline()
        elif ch in (curses.KEY_ENTER, 10, 13, 19):    # Enter / Ctrl+S → confirm
            return "enter"
        elif ch in (KEY_CTRL_ENTER, 24):              # Ctrl+Enter / Ctrl+X → alt confirm
            return "ctrl_enter"
        elif ch in (curses.KEY_BACKSPACE, 127, 8):
            if self.col > 0:
                self.lines[self.row] = self.lines[self.row][:self.col - 1] + self.lines[self.row][self.col:]
                self.col -= 1
            elif self.row > 0:
                prev = self.lines[self.row - 1]
                self.col = len(prev)
                self.lines[self.row - 1] = prev + self.lines[self.row]
                del self.lines[self.row]
                self.row -= 1
        elif ch == curses.KEY_LEFT:
            if self.col > 0:
                self.col -= 1
            elif self.row > 0:
                self.row -= 1
                self.col = len(self.lines[self.row])
        elif ch == curses.KEY_RIGHT:
            if self.col < len(self.lines[self.row]):
                self.col += 1
            elif self.row < len(self.lines) - 1:
                self.row += 1
                self.col = 0
        elif ch == curses.KEY_UP:
            if self.row > 0:
                self.row -= 1
                self.col = min(self.col, len(self.lines[self.row]))
        elif ch == curses.KEY_DOWN:
            if self.row < len(self.lines) - 1:
                self.row += 1
                self.col = min(self.col, len(self.lines[self.row]))
        elif ch == curses.KEY_HOME:
            self.col = 0
        elif ch == curses.KEY_END:
            self.col = len(self.lines[self.row])
        elif 32 <= ch < 127:
            self.lines[self.row] = self.lines[self.row][:self.col] + chr(ch) + self.lines[self.row][self.col:]
            self.col += 1
        return None


def wrap_textarea(ta, width):
    """Soft-wrap a TextArea's logical lines to ``width`` for display only.

    Returns ``(visual_rows, cursor_visual_row, cursor_visual_col)`` where each
    visual row is ``(logical_row, start_col, text)``. No real newline is added,
    so ``ta.text()`` is unaffected.
    """
    width = max(1, width)
    visual = []
    for lrow, line in enumerate(ta.lines):
        rows = len(line) // width + 1  # always ≥1; trailing empty on exact fit
        for k in range(rows):
            start = k * width
            visual.append((lrow, start, line[start:start + width]))
    before = sum(len(ta.lines[r]) // width + 1 for r in range(ta.row))
    cur_vrow = before + ta.col // width
    cur_vcol = ta.col % width
    return visual, cur_vrow, cur_vcol
