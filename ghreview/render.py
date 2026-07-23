"""Curses drawing primitives and the pane renderers."""
import curses
import textwrap

from . import theme
from .models import FOCUS_PRS, FOCUS_COMMITS, FOCUS_PENDING, FOCUS_FILES, FOCUS_DIFF
from .markdown import format_pr_details, wrap_styled
from .navigation import cur_file_path, current_hunk_range, hunk_for_comment
from .tree import files_under_dir


# ---------- primitives ----------

def safe_addstr(win, y, x, s, attr=0, maxw=None):
    """addstr that never raises and never emits an embedded null."""
    if "\x00" in s:
        s = s.replace("\x00", "")
    if maxw is not None:
        s = s[:maxw]
    try:
        win.addstr(y, x, s, attr)
    except curses.error:
        pass


def draw_box(win, y, x, h, w, title, focused, busy=False):
    border_attr = theme.style("focus", bold=True) if focused else curses.A_DIM
    try:
        win.attron(border_attr)
        win.addch(y, x, curses.ACS_ULCORNER)
        win.addch(y, x + w - 1, curses.ACS_URCORNER)
        win.addch(y + h - 1, x, curses.ACS_LLCORNER)
        win.addch(y + h - 1, x + w - 1, curses.ACS_LRCORNER)
        for i in range(1, w - 1):
            win.addch(y, x + i, curses.ACS_HLINE)
            win.addch(y + h - 1, x + i, curses.ACS_HLINE)
        for i in range(1, h - 1):
            win.addch(y + i, x, curses.ACS_VLINE)
            win.addch(y + i, x + w - 1, curses.ACS_VLINE)
        win.attroff(border_attr)
    except curses.error:
        pass
    label = f" {title}{' ⏳' if busy else ''} "
    title_attr = theme.style("focus", bold=True) if focused else theme.style("title")
    safe_addstr(win, y, x + 2, label, title_attr, maxw=max(0, w - 4))


def draw_scrollbar(win, y, x, h, view_offset, view_h, total, focused=False):
    if total <= view_h or h <= 0:
        return
    bar_h = max(1, view_h * h // total)
    span = max(1, total - view_h)
    bar_y = view_offset * (h - bar_h) // span
    bar_attr = theme.style("focus", bold=True) if focused else curses.A_DIM
    for i in range(h):
        on = bar_y <= i < bar_y + bar_h
        try:
            win.addstr(y + i, x, "█" if on else "░", bar_attr if on else curses.A_DIM)
        except curses.error:
            pass


def clamp_view(idx, view_offset, view_h, total):
    """Return a view offset that keeps ``idx`` visible within ``view_h`` rows."""
    if total == 0:
        return 0
    if idx < view_offset:
        return idx
    if idx >= view_offset + view_h:
        return idx - view_h + 1
    if view_offset > max(0, total - view_h):
        return max(0, total - view_h)
    return view_offset


def reveal_scroll(scroll, lo, hi, vh):
    """Minimal scroll offset so the range ``[lo, hi)`` is visible in ``vh`` rows.

    Returns ``scroll`` unchanged when the range is already fully on screen;
    otherwise scrolls just enough. A range taller than the viewport aligns to
    its top (``lo``).
    """
    if vh <= 0:
        return scroll
    if lo < scroll:
        return lo
    if hi > scroll + vh:
        return lo if (hi - lo) > vh else hi - vh
    return scroll


def selection_attr():
    """Attribute for the focused, selected row."""
    return theme.style("sel", bold=True)


# ---------- shortcut hints ----------

def shortcuts_for(st):
    common = "0-4: focus pane · Tab: next · f: finish review · r: refresh · q: quit"
    if st.focus == FOCUS_PRS:
        return f"Enter: open (worktree) · j/k: move · Shift+J/K: scroll summary · {common}"
    if st.focus == FOCUS_COMMITS:
        return (f"Space: toggle · a: all/none · Enter: apply range · j/k: move · {common}")
    if st.focus == FOCUS_PENDING:
        return f"Enter: submit review · e: edit · d: delete · j/k: move · {common}"
    if st.focus == FOCUS_FILES:
        return (f"Enter: open/collapse · Space: viewed · e: editor · z: fold viewed · "
                f"j/k: move · Alt+j/k: next/prev file · {common}")
    return f"j/k: next/prev hunk · c: comment · e: editor · PgUp/PgDn: scroll · Esc: back · {common}"


# ---------- left column: PRs / Commits / Files / Pending ----------

PR_SECTION_LABELS = {"mine": "My PRs", "review": "Requested review"}


def pr_rows(prs):
    """Display rows for the PRs pane: section headers + PR indices.

    Returns a list of ``("hdr", label)`` and ``("pr", index)`` tuples. Assumes
    ``prs`` is grouped so all "mine" precede all "review".
    """
    rows = []
    last_cat = None
    for i, pr in enumerate(prs):
        cat = getattr(pr, "category", "mine")
        if cat != last_cat:
            rows.append(("hdr", PR_SECTION_LABELS.get(cat, cat)))
            last_cat = cat
        rows.append(("pr", i))
    return rows


def render_prs(stdscr, st, y, x, h, w):
    draw_box(stdscr, y, x, h, w, f"[1] PRs [{st.repo_owner}/{st.repo_name}]",
             st.focus == FOCUS_PRS, busy="prs" in st.busy)
    vh, iw = h - 2, w - 3
    counts = {}
    for p in st.prs:
        counts[getattr(p, "category", "mine")] = counts.get(getattr(p, "category", "mine"), 0) + 1
    rows = pr_rows(st.prs)
    # Scroll so the selected PR's row stays visible (headers count as rows).
    sel_row = next((r for r, (k, v) in enumerate(rows) if k == "pr" and v == st.pr_idx), 0)
    st.pr_view_offset = clamp_view(sel_row, st.pr_view_offset, vh, len(rows))
    for i, (kind, val) in enumerate(rows[st.pr_view_offset:st.pr_view_offset + vh]):
        row_y = y + 1 + i
        if kind == "hdr":
            cat = "mine" if val == PR_SECTION_LABELS["mine"] else "review"
            safe_addstr(stdscr, row_y, x + 1, f"{val} ({counts.get(cat, 0)})".ljust(iw),
                        theme.style("keys", dim=True, bold=True), maxw=iw)
            continue
        pr = st.prs[val]
        active = st.active_pr and st.active_pr.number == pr.number
        line = f"{'● ' if active else '  '}#{pr.number} {pr.title}"
        if val == st.pr_idx and st.focus == FOCUS_PRS:
            attr = selection_attr()
        elif active:
            attr = theme.style("active", bold=True)
        else:
            attr = 0
        safe_addstr(stdscr, row_y, x + 1, line.ljust(iw), attr, maxw=iw)
    draw_scrollbar(stdscr, y + 1, x + w - 2, vh, st.pr_view_offset, vh, len(rows),
                   st.focus == FOCUS_PRS)


def render_commits(stdscr, st, y, x, h, w):
    n_sel = len(st.commit_selected)
    total = len(st.commits)
    title = f"[2] Commits ({n_sel}/{total})" if total else "[2] Commits"
    draw_box(stdscr, y, x, h, w, title, st.focus == FOCUS_COMMITS,
             busy="active" in st.busy or "commitdiff" in st.busy)
    vh, iw = h - 2, w - 3
    if not st.commits:
        safe_addstr(stdscr, y + 1, x + 2, "No commits", curses.A_DIM, maxw=iw)
        return
    st.commit_view_offset = clamp_view(st.commit_idx, st.commit_view_offset, vh, len(st.commits))
    for i, c in enumerate(st.commits[st.commit_view_offset:st.commit_view_offset + vh]):
        idx = st.commit_view_offset + i
        checked = c.oid in st.commit_selected
        line = f"[{'x' if checked else ' '}] {c.short} {c.headline}"
        if idx == st.commit_idx and st.focus == FOCUS_COMMITS:
            attr = selection_attr()
        elif not checked:
            attr = curses.A_DIM
        else:
            attr = 0
        safe_addstr(stdscr, y + 1 + i, x + 1, line.ljust(iw), attr, maxw=iw)
    draw_scrollbar(stdscr, y + 1, x + w - 2, vh, st.commit_view_offset, vh, len(st.commits),
                   st.focus == FOCUS_COMMITS)


def render_pending(stdscr, st, y, x, h, w):
    draw_box(stdscr, y, x, h, w, f"[4] Pending ({len(st.pending)})",
             st.focus == FOCUS_PENDING, busy=bool({"review", "pending"} & st.busy))
    vh, iw = h - 2, w - 3
    if not st.pending:
        safe_addstr(stdscr, y + 1, x + 2, "No pending comments", curses.A_DIM, maxw=iw)
        return
    st.pending_view_offset = clamp_view(st.pending_idx, st.pending_view_offset, vh, len(st.pending))
    for i, c in enumerate(st.pending[st.pending_view_offset:st.pending_view_offset + vh]):
        idx = st.pending_view_offset + i
        first_line = c.body.splitlines()[0] if c.body else ""
        line = f"{c.path}:{c.line}  {first_line}"
        attr = selection_attr() if (idx == st.pending_idx and st.focus == FOCUS_PENDING) else 0
        safe_addstr(stdscr, y + 1 + i, x + 1, line.ljust(iw), attr, maxw=iw)
    draw_scrollbar(stdscr, y + 1, x + w - 2, vh, st.pending_view_offset, vh, len(st.pending),
                   st.focus == FOCUS_PENDING)


def render_files(stdscr, st, y, x, h, w):
    files_title = "[3] Files"
    if st.active_pr:
        n_viewed = sum(1 for f in st.files if f.viewed)
        files_title = f"[3] Files #{st.active_pr.number}  {n_viewed}/{len(st.files)} viewed"
    draw_box(stdscr, y, x, h, w, files_title,
             st.focus == FOCUS_FILES, busy="active" in st.busy or "viewed" in st.busy)
    vh, iw = h - 2, w - 3
    st.file_view_offset = clamp_view(st.file_idx, st.file_view_offset, vh, len(st.tree))
    for i, item in enumerate(st.tree[st.file_view_offset:st.file_view_offset + vh]):
        idx = st.file_view_offset + i
        depth, name, kind, payload, *rest = (*item, None)[:5]
        indent = "  " * depth
        summary = ""
        if kind == "dir":
            is_collapsed = rest[0] if rest else False
            dir_files = files_under_dir(st, payload)
            if dir_files:
                summary = f"{sum(1 for f in dir_files if f.viewed)}/{len(dir_files)}"
            line = f"{indent}{'▶' if is_collapsed else '▼'} {name}/"
            base = curses.A_BOLD
        else:
            fe = st.files[payload]
            line = f"{indent}[{'✔' if fe.viewed else ' '}] {name}"
            base = theme.style("add", dim=True) if fe.viewed else 0
        attr = selection_attr() if (idx == st.file_idx and st.focus == FOCUS_FILES) else base
        row = y + 1 + i
        safe_addstr(stdscr, row, x + 1, line.ljust(iw), attr, maxw=iw)
        if summary:
            summary_attr = (attr & ~curses.A_BOLD) | curses.A_DIM
            sx = x + 1 + iw - len(summary)
            if sx > x + 1 + len(line):
                safe_addstr(stdscr, row, sx, summary, summary_attr, maxw=iw - (sx - x - 1))
    draw_scrollbar(stdscr, y + 1, x + w - 2, vh, st.file_view_offset, vh, len(st.tree),
                   st.focus == FOCUS_FILES)


# ---------- right pane ----------

def _draw_diff_line(stdscr, y, x, iw, line, current, marker_attr, selected=False):
    """Draw one diff line with the current-hunk marker / comment selection."""
    attr = theme.diff_line_style(line, current)
    if selected:
        attr |= curses.A_REVERSE
    text = line.expandtabs(4).ljust(iw - 1)
    if selected:
        marker, m_attr = "▶", theme.style("focus", bold=True)
    elif current:
        marker, m_attr = "▌", marker_attr
    else:
        marker, m_attr = " ", 0
    safe_addstr(stdscr, y, x + 1, marker, m_attr)
    safe_addstr(stdscr, y, x + 2, text, attr, maxw=iw - 1)


def render_diff(stdscr, st, y, x, h, w):
    path = cur_file_path(st)
    draw_box(stdscr, y, x, h, w, f"[0] Diff — {path}" if path else "[0] Diff",
             st.focus == FOCUS_DIFF)
    diff_lines = st.diff_by_file.get(path, []) if path else []
    if not diff_lines and path:
        diff_lines = ["(no diff — file may be binary, removed, or too large)"]
    vh, iw = h - 2, w - 3
    # Only highlight the current hunk while the diff pane itself is focused.
    focused = st.focus == FOCUS_DIFF
    cur_hr = current_hunk_range(st, path) if (path and focused) else None
    # Scroll just enough to reveal the focus — the comment cursor while picking,
    # otherwise the selected block. No scroll if it's already fully on screen,
    # so navigating to an already-visible hunk doesn't yank it to the top.
    if st.comment_mode and path:
        st.diff_scroll = reveal_scroll(st.diff_scroll, st.comment_line,
                                       st.comment_line + 1, vh)
    elif cur_hr:
        st.diff_scroll = reveal_scroll(st.diff_scroll, cur_hr[0], cur_hr[1], vh)
    st.diff_scroll = max(0, min(st.diff_scroll, max(0, len(diff_lines) - vh)))
    marker_attr = theme.hunk_marker_style()
    # Diff-line index range the comment picker currently spans.
    if st.comment_mode:
        anchor = st.comment_start if st.comment_start is not None else st.comment_line
        sel_lo, sel_hi = sorted((anchor, st.comment_line))
    else:
        sel_lo, sel_hi = 1, 0  # empty range
    for i, ln in enumerate(diff_lines[st.diff_scroll:st.diff_scroll + vh]):
        line_idx = st.diff_scroll + i
        current = bool(cur_hr and cur_hr[0] <= line_idx < cur_hr[1])
        selected = sel_lo <= line_idx <= sel_hi
        _draw_diff_line(stdscr, y + 1 + i, x, iw, ln, current, marker_attr, selected)
    draw_scrollbar(stdscr, y + 1, x + w - 2, vh, st.diff_scroll, vh, len(diff_lines),
                   st.focus == FOCUS_DIFF)


def render_pending_detail(stdscr, st, y, x, h, w):
    draw_box(stdscr, y, x, h, w, "Pending comment", st.focus == FOCUS_PENDING)
    vh, iw = h - 2, w - 3
    if not st.pending:
        safe_addstr(stdscr, y + 1, x + 2, "No pending comments", curses.A_DIM, maxw=iw)
        return
    c = st.pending[min(max(0, st.pending_idx), len(st.pending) - 1)]
    hunk_lines, target = hunk_for_comment(st, c)
    row, bottom = y + 1, y + 1 + vh
    safe_addstr(stdscr, row, x + 2, f"{c.path}:{c.line}", theme.style("title", bold=True), maxw=iw - 1)
    row += 1
    if hunk_lines:
        marker_attr = theme.style("curhunk", bold=True)
        for j, ln in enumerate(hunk_lines):
            if row >= bottom:
                break
            attr = theme.diff_line_style(ln)
            is_target = j == target
            safe_addstr(stdscr, row, x + 1, "▌" if is_target else " ",
                        marker_attr if is_target else 0)
            safe_addstr(stdscr, row, x + 2, ln.expandtabs(4), attr, maxw=iw - 1)
            row += 1
    else:
        safe_addstr(stdscr, row, x + 2, "(hunk not in current diff)", curses.A_DIM, maxw=iw - 1)
        row += 1
    if row < bottom:
        safe_addstr(stdscr, row, x + 1, "─" * (w - 2), curses.A_DIM)
        row += 1
    if row < bottom:
        safe_addstr(stdscr, row, x + 2, "Comment:", theme.style("focus", bold=True), maxw=iw - 1)
        row += 1
    for bl in (c.body.splitlines() or [""]):
        for seg in (textwrap.wrap(bl, iw - 1) or [""]):
            if row >= bottom:
                break
            safe_addstr(stdscr, row, x + 2, seg, 0, maxw=iw - 1)
            row += 1
        if row >= bottom:
            break


def render_commit_detail(stdscr, st, y, x, h, w):
    draw_box(stdscr, y, x, h, w, "Commit", st.focus == FOCUS_COMMITS)
    vh, iw = h - 2, w - 3
    if not st.commits:
        safe_addstr(stdscr, y + 1, x + 2, "No commits", curses.A_DIM, maxw=iw)
        return
    c = st.commits[min(max(0, st.commit_idx), len(st.commits) - 1)]
    row, bottom = y + 1, y + 1 + vh
    header = [
        (f"{c.short}  {c.headline}", theme.style("title", bold=True)),
        (f"{c.author}   {c.date}", curses.A_DIM),
        ("", 0),
    ]
    for text, attr in header:
        if row >= bottom:
            return
        safe_addstr(stdscr, row, x + 2, text, attr, maxw=iw - 1)
        row += 1
    for bl in (c.body.splitlines() if c.body else []):
        for seg in (textwrap.wrap(bl, iw - 1) or [""]):
            if row >= bottom:
                return
            safe_addstr(stdscr, row, x + 2, seg, 0, maxw=iw - 1)
            row += 1


def render_pr_details(stdscr, st, y, x, h, w):
    pr = st.prs[st.pr_idx] if st.prs and 0 <= st.pr_idx < len(st.prs) else None
    if not pr:
        draw_box(stdscr, y, x, h, w, "PR details", False)
        return
    data = st.pr_details.get(pr.number)
    draw_box(stdscr, y, x, h, w, f"PR #{pr.number} · {pr.title}", False, busy=data is None)
    vh, iw = h - 2, w - 3
    if data is None:
        safe_addstr(stdscr, y + 1, x + 2, "Loading…", curses.A_DIM, maxw=iw)
        return
    lines = wrap_styled(format_pr_details(data), iw - 1)
    st.details_scroll = min(st.details_scroll, max(0, len(lines) - vh))
    styles = theme.detail_styles()
    for i, (ln, kind) in enumerate(lines[st.details_scroll:st.details_scroll + vh]):
        text = ln.expandtabs(4).ljust(iw - 1)
        safe_addstr(stdscr, y + 1 + i, x + 2, text, styles.get(kind, 0), maxw=iw - 1)
    draw_scrollbar(stdscr, y + 1, x + w - 2, vh, st.details_scroll, vh, len(lines), False)


# ---------- layout ----------

def compute_layout(H, W):
    """Return the rect dict for the five panes given the screen size."""
    left_w = max(38, W // 3)
    right_w = W - left_w
    body_h = H - 2
    pr_h = max(6, body_h * 2 // 9)
    commits_h = max(4, body_h // 6)
    pending_h = max(3, body_h // 8)
    files_h = body_h - pr_h - commits_h - pending_h
    return {
        "prs": (0, 0, pr_h, left_w),
        "commits": (pr_h, 0, commits_h, left_w),
        "files": (pr_h + commits_h, 0, files_h, left_w),
        "pending": (pr_h + commits_h + files_h, 0, pending_h, left_w),
        "right": (0, left_w, body_h, right_w),
    }


def render(stdscr, st):
    stdscr.erase()
    H, W = stdscr.getmaxyx()
    st.rects = compute_layout(H, W)
    py, px, ph, pw = st.rects["prs"]
    render_prs(stdscr, st, py, px, ph, pw)
    py, px, ph, pw = st.rects["commits"]
    render_commits(stdscr, st, py, px, ph, pw)
    py, px, ph, pw = st.rects["pending"]
    render_pending(stdscr, st, py, px, ph, pw)
    py, px, ph, pw = st.rects["files"]
    render_files(stdscr, st, py, px, ph, pw)
    ry, rx, rh, rw = st.rects["right"]
    if st.focus == FOCUS_PRS:
        render_pr_details(stdscr, st, ry, rx, rh, rw)
    elif st.focus == FOCUS_COMMITS:
        render_commit_detail(stdscr, st, ry, rx, rh, rw)
    elif st.focus == FOCUS_PENDING:
        render_pending_detail(stdscr, st, ry, rx, rh, rw)
    else:
        render_diff(stdscr, st, ry, rx, rh, rw)

    status = st.status or "Ready."
    safe_addstr(stdscr, H - 2, 0, status.ljust(W - 1), theme.style("status", bold=True), maxw=W - 1)
    safe_addstr(stdscr, H - 1, 0, shortcuts_for(st).ljust(W - 1), theme.style("keys", dim=True), maxw=W - 1)
    stdscr.refresh()
