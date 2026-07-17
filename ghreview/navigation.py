"""Cursor / hunk / selection logic over ``State`` (pure, no curses)."""
from .diff import HUNK_RE


def cur_file_path(st):
    """Path of the file currently selected in the tree, or None (dir/empty)."""
    if 0 <= st.file_idx < len(st.tree) and st.tree[st.file_idx][2] == "file":
        return st.files[st.tree[st.file_idx][3]].path
    return None


def current_hunk_range(st, path):
    """``(start, end)`` of the selected hunk for ``path``, or None."""
    hunks = st.hunks_by_file.get(path, [])
    if not hunks:
        return None
    idx = max(0, min(len(hunks) - 1, st.diff_hunk_idx))
    return hunks[idx]


def line_target(st, path, idx):
    """``(line_no, side)`` a comment on diff-line ``idx`` should attach to.

    Added → new/RIGHT, deleted → old/LEFT, context → new/RIGHT. Returns None for
    non-line rows (headers, "\\ No newline").
    """
    info = st.info_by_file.get(path, [])
    if not (0 <= idx < len(info)):
        return None
    old, new = info[idx]
    if new is not None and old is None:
        return (new, "RIGHT")
    if old is not None and new is None:
        return (old, "LEFT")
    if new is not None:
        return (new, "RIGHT")
    if old is not None:
        return (old, "LEFT")
    return None


def hunk_line_indices(st, path):
    """Diff-line indices in the current hunk that a comment can attach to."""
    hr = current_hunk_range(st, path)
    info = st.info_by_file.get(path, [])
    if not hr:
        return []
    s, e = hr
    return [i for i in range(s + 1, e)
            if i < len(info) and info[i] != (None, None)]


def first_change_index(st, path):
    """Diff-line index of the first added/deleted line in the current hunk."""
    hr = current_hunk_range(st, path)
    info = st.info_by_file.get(path, [])
    if hr:
        s, e = hr
        for i in range(s + 1, e):
            if i < len(info):
                old, new = info[i]
                if (new is not None) != (old is not None):  # exactly one side
                    return i
    idxs = hunk_line_indices(st, path)
    return idxs[0] if idxs else None


def scroll_diff(st, delta):
    st.diff_scroll = max(0, st.diff_scroll + delta)


def jump_hunk(st, direction):
    """Select the next/previous hunk and scroll to keep it visible.

    The selected hunk is tracked independently of the scroll offset so it can
    advance even when the whole diff already fits in the viewport.
    """
    path = cur_file_path(st)
    hunks = st.hunks_by_file.get(path, []) if path else []
    if not hunks:
        return
    idx = max(0, min(len(hunks) - 1, st.diff_hunk_idx + direction))
    st.diff_hunk_idx = idx
    st.diff_scroll = hunks[idx][0]


def current_hunk_editor_line(st, path):
    """New-file line to open in the editor for the selected hunk.

    Prefer the first *added* line (``+``); fall back to the first line present on
    the new side, then to the hunk header's new start.
    """
    hr = current_hunk_range(st, path)
    info = st.info_by_file.get(path, [])
    if hr:
        s, e = hr
        for i in range(s, e):  # first added line: new side only
            if i < len(info):
                old, new = info[i]
                if new is not None and old is None:
                    return new
        for i in range(s, e):  # else first new-side line (pure deletion)
            if i < len(info) and info[i][1] is not None:
                return info[i][1]
        lines = st.diff_by_file.get(path, [])
        if s < len(lines):
            m = HUNK_RE.match(lines[s])
            if m:
                return int(m.group(3))
    return 1


def hunk_for_comment(st, c):
    """``(hunk_lines, target_offset)`` for the hunk a pending comment anchors to."""
    lines = st.diff_by_file.get(c.path, [])
    info = st.info_by_file.get(c.path, [])
    hunks = st.hunks_by_file.get(c.path, [])
    target = None
    for i, (old, new) in enumerate(info):
        if (c.side != "LEFT" and new == c.line) or (c.side == "LEFT" and old == c.line):
            target = i
            break
    for (s, e) in hunks:
        if target is not None and s <= target < e:
            return lines[s:e], target - s
    return [], None
