"""Curses bootstrap, the main event loop, and the entry point."""
import curses
import os
import queue
import sys
import threading

from . import api, theme
from .gh import sh
from .keys import get_key, disable_flow_control, KEY_ALT_J, KEY_ALT_K
from .models import (State, N_PANES, FOCUS_PRS, FOCUS_COMMITS, FOCUS_PENDING,
                     FOCUS_FILES, FOCUS_DIFF)
from .navigation import (scroll_diff, jump_hunk, cur_file_path,
                         hunk_line_indices, first_change_index)
from .tree import rebuild_tree, files_under_dir, fold_viewed_dirs, first_unviewed_index
from .editor import open_current_in_editor
from .modals import curs_set
from .render import render
from .worker import worker_loop
from .controller import (
    submit_job, apply_result, maybe_load_details, pane_at,
    apply_commit_selection, open_comment_modal, open_finish_modal,
    open_edit_pending_modal,
)

FOCUS_BY_DIGIT = {ord("0"): FOCUS_DIFF, ord("1"): FOCUS_PRS,
                  ord("2"): FOCUS_COMMITS, ord("3"): FOCUS_FILES,
                  ord("4"): FOCUS_PENDING}
FOCUS_BY_PANE = {"prs": FOCUS_PRS, "commits": FOCUS_COMMITS,
                 "pending": FOCUS_PENDING, "files": FOCUS_FILES,
                 "right": FOCUS_DIFF}


def _toggle_collapse(st, path):
    (st.collapsed_dirs.discard if path in st.collapsed_dirs
     else st.collapsed_dirs.add)(path)
    rebuild_tree(st)


def _handle_mouse(st, jobs):
    try:
        _, mx, my, _, bstate = curses.getmouse()
    except curses.error:
        return
    pane = pane_at(st, my, mx)
    if pane is None:
        return
    up = bool(bstate & curses.BUTTON4_PRESSED)
    down = hasattr(curses, "BUTTON5_PRESSED") and bool(bstate & curses.BUTTON5_PRESSED)
    if not up and not down and bstate & curses.BUTTON1_CLICKED:
        st.focus = FOCUS_BY_PANE[pane]
        return
    step = 3
    vh = max(1, st.rects[pane][2] - 2)
    if pane == "prs":
        maxoff = max(0, len(st.prs) - vh)
        st.pr_view_offset = (max(0, st.pr_view_offset - step) if up
                             else min(maxoff, st.pr_view_offset + step) if down else st.pr_view_offset)
    elif pane == "commits":
        maxoff = max(0, len(st.commits) - vh)
        st.commit_view_offset = (max(0, st.commit_view_offset - step) if up
                                 else min(maxoff, st.commit_view_offset + step) if down else st.commit_view_offset)
    elif pane == "pending":
        maxoff = max(0, len(st.pending) - vh)
        st.pending_view_offset = (max(0, st.pending_view_offset - step) if up
                                  else min(maxoff, st.pending_view_offset + step) if down else st.pending_view_offset)
    elif pane == "files":
        maxoff = max(0, len(st.tree) - vh)
        st.file_view_offset = (max(0, st.file_view_offset - step) if up
                               else min(maxoff, st.file_view_offset + step) if down else st.file_view_offset)
    elif pane == "right":
        if st.focus == FOCUS_PRS:
            if up:
                st.details_scroll = max(0, st.details_scroll - step)
            elif down:
                st.details_scroll += step
        else:
            if up:
                scroll_diff(st, -step)
            elif down:
                scroll_diff(st, step)


def _mark_viewed(st, jobs):
    if not (0 <= st.file_idx < len(st.tree)) or not st.active_pr:
        return
    item = st.tree[st.file_idx]
    if item[2] == "file":
        fe = st.files[item[3]]
        new_v = not fe.viewed
        st.status = f"{'Marking' if new_v else 'Unmarking'} {fe.path}…"
        submit_job(jobs, st, ("mark_viewed", st.active_pr.node_id, fe.path, new_v))
    else:
        dir_files = files_under_dir(st, item[3])
        if dir_files:
            new_v = not all(f.viewed for f in dir_files)
            paths = [f.path for f in dir_files if f.viewed != new_v]
            if paths:
                st.status = f"{'Marking' if new_v else 'Unmarking'} {len(paths)} files in {item[3]}/…"
                submit_job(jobs, st, ("mark_viewed_bulk", st.active_pr.node_id, paths, new_v))


def _fold_viewed(st):
    folded = fold_viewed_dirs(st)
    ti = first_unviewed_index(st)
    if ti is not None:
        st.file_idx = ti
        st.diff_scroll = 0
        st.diff_hunk_idx = 0
    st.status = (f"Folded {folded} viewed folder{'s' if folded != 1 else ''}"
                 + (" · jumped to first unviewed file" if ti is not None else " · no unviewed files"))


def _toggle_commit(st):
    if 0 <= st.commit_idx < len(st.commits):
        oid = st.commits[st.commit_idx].oid
        (st.commit_selected.discard if oid in st.commit_selected
         else st.commit_selected.add)(oid)


def _toggle_all_commits(st):
    if len(st.commit_selected) == len(st.commits):
        st.commit_selected = set()
    else:
        st.commit_selected = {c.oid for c in st.commits}


def _jump_file(st, direction):
    """Move the tree cursor to the next/previous *file* row, skipping folders."""
    i = st.file_idx + direction
    while 0 <= i < len(st.tree):
        if st.tree[i][2] == "file":
            st.file_idx = i
            st.diff_scroll = st.diff_hunk_idx = 0
            return
        i += direction


def _enter_comment_mode(st):
    """Start the in-hunk line picker for a comment."""
    if not st.active_pr:
        st.status = "No active PR."
        return
    path = cur_file_path(st)
    idxs = hunk_line_indices(st, path) if path else []
    if not idxs:
        st.status = "No commentable line in the current hunk."
        return
    st.comment_mode = True
    st.comment_start = None
    fc = first_change_index(st, path)
    st.comment_line = fc if fc is not None else idxs[0]
    st.status = "Comment: j/k line · Shift+J/K range · Enter confirm · Esc cancel"


def _move_comment(st, path, direction, extend):
    idxs = hunk_line_indices(st, path)
    if not idxs:
        return
    if st.comment_line not in idxs:
        st.comment_line = idxs[0]
    pos = idxs.index(st.comment_line)
    pos = max(0, min(len(idxs) - 1, pos + direction))
    if extend:
        if st.comment_start is None:
            st.comment_start = st.comment_line  # anchor the range before moving
    else:
        st.comment_start = None
    st.comment_line = idxs[pos]


def _handle_comment_mode(stdscr, st, jobs, ch):
    """Keys while picking a line/range for a comment (captures navigation)."""
    path = cur_file_path(st)
    if ch == 27:
        st.comment_mode = False
        st.status = "Comment cancelled."
    elif ch in (curses.KEY_DOWN, ord("j")):
        _move_comment(st, path, +1, extend=False)
    elif ch in (curses.KEY_UP, ord("k")):
        _move_comment(st, path, -1, extend=False)
    elif ch == ord("J"):
        _move_comment(st, path, +1, extend=True)
    elif ch == ord("K"):
        _move_comment(st, path, -1, extend=True)
    elif ch in (curses.KEY_ENTER, 10, 13):
        try:
            open_comment_modal(stdscr, st, jobs)
        except Exception as e:
            curs_set(0)
            st.comment_mode = False
            st.status = f"comment error: {type(e).__name__}: {e}"
    return True


def _open_file_or_dir(st):
    if not (0 <= st.file_idx < len(st.tree)):
        return
    item = st.tree[st.file_idx]
    if item[2] == "file":
        st.focus = FOCUS_DIFF
        st.diff_scroll = 0
        st.diff_hunk_idx = 0
    else:
        _toggle_collapse(st, item[3])


def _handle_key(stdscr, st, jobs, ch):
    """Dispatch a key. Returns True to keep running, False to quit."""
    # The in-hunk comment picker is modal: it captures navigation first.
    if st.comment_mode:
        return _handle_comment_mode(stdscr, st, jobs, ch)
    # --- global ---
    if ch == ord("q"):
        return False
    if ch in FOCUS_BY_DIGIT:
        st.focus = FOCUS_BY_DIGIT[ch]
        return True
    if ch == ord("J"):
        if st.focus == FOCUS_PRS:
            st.details_scroll += 1
        else:
            scroll_diff(st, 1)
        return True
    if ch == ord("K"):
        if st.focus == FOCUS_PRS:
            st.details_scroll = max(0, st.details_scroll - 1)
        else:
            scroll_diff(st, -1)
        return True
    if ch == 9:  # Tab
        st.focus = (st.focus + 1) % N_PANES
        return True
    if ch == curses.KEY_BTAB:
        st.focus = (st.focus - 1) % N_PANES
        return True
    if ch == ord("r"):
        if "prs" not in st.busy:
            submit_job(jobs, st, ("load_prs",))
        # Re-fetch the active PR's worktree (picks up new pushes) and reload it.
        if st.active_pr and not ({"worktree", "active"} & st.busy):
            submit_job(jobs, st, ("open_pr", st.repo_root, st.repo_owner,
                                  st.repo_name, st.active_pr.number))
        if st.focus == FOCUS_PRS and st.prs:
            st.pr_details.pop(st.prs[st.pr_idx].number, None)
            maybe_load_details(st, jobs)
        st.status = "Refreshing…"
        return True
    if ch == ord("f"):
        try:
            open_finish_modal(stdscr, st, jobs)
        except Exception as e:
            curs_set(0)
            st.status = f"finish-review error: {type(e).__name__}: {e}"
        return True
    if ch == curses.KEY_MOUSE:
        _handle_mouse(st, jobs)
        return True

    # --- pane-scoped ---
    if st.focus == FOCUS_PRS:
        if ch in (curses.KEY_DOWN, ord("j")):
            st.pr_idx = min(max(0, len(st.prs) - 1), st.pr_idx + 1)
        elif ch in (curses.KEY_UP, ord("k")):
            st.pr_idx = max(0, st.pr_idx - 1)
        elif ch in (curses.KEY_NPAGE, ord("d")):
            st.details_scroll += 10
        elif ch in (curses.KEY_PPAGE, ord("u")):
            st.details_scroll = max(0, st.details_scroll - 10)
        elif ch in (curses.KEY_ENTER, 10, 13):
            if st.prs and not ({"worktree", "active"} & st.busy):
                pr = st.prs[st.pr_idx]
                st.status = f"Opening #{pr.number} in a worktree…"
                submit_job(jobs, st, ("open_pr", st.repo_root, st.repo_owner,
                                      st.repo_name, pr.number))
    elif st.focus == FOCUS_COMMITS:
        if ch in (curses.KEY_DOWN, ord("j")):
            st.commit_idx = min(max(0, len(st.commits) - 1), st.commit_idx + 1)
        elif ch in (curses.KEY_UP, ord("k")):
            st.commit_idx = max(0, st.commit_idx - 1)
        elif ch == ord(" "):
            _toggle_commit(st)
        elif ch == ord("a"):
            _toggle_all_commits(st)
        elif ch in (curses.KEY_ENTER, 10, 13):
            apply_commit_selection(st, jobs)
    elif st.focus == FOCUS_PENDING:
        if ch in (curses.KEY_DOWN, ord("j")):
            st.pending_idx = min(max(0, len(st.pending) - 1), st.pending_idx + 1)
        elif ch in (curses.KEY_UP, ord("k")):
            st.pending_idx = max(0, st.pending_idx - 1)
        elif ch in (curses.KEY_ENTER, 10, 13):
            try:
                open_finish_modal(stdscr, st, jobs)
            except Exception as e:
                curs_set(0)
                st.status = f"finish-review error: {type(e).__name__}: {e}"
        elif ch == ord("e"):
            if "pending" not in st.busy:
                try:
                    open_edit_pending_modal(stdscr, st, jobs)
                except Exception as e:
                    curs_set(0)
                    st.status = f"edit error: {type(e).__name__}: {e}"
        elif ch == ord("d"):
            if 0 <= st.pending_idx < len(st.pending) and "pending" not in st.busy and st.active_pr:
                removed = st.pending.pop(st.pending_idx)
                st.pending_idx = min(st.pending_idx, max(0, len(st.pending) - 1))
                submit_job(jobs, st, ("discard_pending", st.repo_owner, st.repo_name,
                                      st.active_pr.number, st.viewer, removed.comment_id))
                st.status = f"Discarding comment on {removed.path}:{removed.line}…"
    elif st.focus == FOCUS_FILES:
        if ch in (curses.KEY_DOWN, ord("j")):
            st.file_idx = min(max(0, len(st.tree) - 1), st.file_idx + 1)
            st.diff_scroll = st.diff_hunk_idx = 0
        elif ch in (curses.KEY_UP, ord("k")):
            st.file_idx = max(0, st.file_idx - 1)
            st.diff_scroll = st.diff_hunk_idx = 0
        elif ch == KEY_ALT_J:
            _jump_file(st, +1)
        elif ch == KEY_ALT_K:
            _jump_file(st, -1)
        elif ch in (curses.KEY_ENTER, 10, 13):
            _open_file_or_dir(st)
        elif ch == ord(" "):
            _mark_viewed(st, jobs)
        elif ch == ord("z"):
            _fold_viewed(st)
        elif ch == ord("e"):
            open_current_in_editor(st, top=True)
    elif st.focus == FOCUS_DIFF:
        H, _ = stdscr.getmaxyx()
        page = max(1, H - 4)
        path = cur_file_path(st)
        diff_lines = st.diff_by_file.get(path, []) if path else []
        max_scroll = max(0, len(diff_lines) - 1)
        if ch in (curses.KEY_DOWN, ord("j")):
            jump_hunk(st, +1)
        elif ch in (curses.KEY_UP, ord("k")):
            jump_hunk(st, -1)
        elif ch == curses.KEY_NPAGE:
            st.diff_scroll = min(max_scroll, st.diff_scroll + page)
        elif ch == curses.KEY_PPAGE:
            st.diff_scroll = max(0, st.diff_scroll - page)
        elif ch == ord("c"):
            _enter_comment_mode(st)
        elif ch == ord("e"):
            open_current_in_editor(st)
        elif ch == 27:
            st.focus = FOCUS_FILES
    return True


def run(stdscr):
    curses.curs_set(0)
    theme.init()
    curses.mousemask(curses.ALL_MOUSE_EVENTS)
    try:
        curses.set_escdelay(25)
    except (AttributeError, curses.error):
        os.environ.setdefault("ESCDELAY", "25")
    disable_flow_control()
    try:
        # Mouse tracking + modifyOtherKeys level 1 (distinguish Shift/Ctrl+Enter).
        print("\033[?1000h\033[?1006h\033[>4;1m", end="", flush=True)
    except Exception:
        pass
    stdscr.timeout(80)

    jobs: queue.Queue = queue.Queue()
    results: queue.Queue = queue.Queue()
    threading.Thread(target=worker_loop, args=(jobs, results), daemon=True).start()

    st = State()
    try:
        st.repo_owner, st.repo_name = api.detect_repo()
    except Exception as e:
        st.status = f"detect_repo: {e}"
    st.repo_root = api.get_repo_root()
    st.viewer = api.get_viewer_login()
    if st.repo_owner:
        # Load the PR list; a PR is opened (into a worktree) on demand so the
        # main checkout is never touched.
        submit_job(jobs, st, ("load_prs",))
        # Reopen the PR reviewed last time (rebuilds its worktree), if any.
        last = api.load_last_pr(st.repo_owner, st.repo_name)
        if last is not None:
            st.status = f"Reopening #{last} from last session…"
            submit_job(jobs, st, ("open_pr", st.repo_root, st.repo_owner,
                                  st.repo_name, last))

    prev_pr_idx = prev_focus = -1
    running = True
    while running:
        while True:
            try:
                apply_result(st, results.get_nowait(), jobs)
            except queue.Empty:
                break

        if st.focus == FOCUS_PRS:
            if st.pr_idx != prev_pr_idx or prev_focus != FOCUS_PRS:
                st.details_scroll = 0
            maybe_load_details(st, jobs)
        prev_pr_idx, prev_focus = st.pr_idx, st.focus

        try:
            render(stdscr, st)
        except Exception as e:
            st.status = f"render error: {type(e).__name__}: {e}"

        ch = get_key(stdscr)
        if ch == -1:
            continue
        try:
            running = _handle_key(stdscr, st, jobs, ch)
        except Exception as e:
            curs_set(0)
            st.status = f"error: {type(e).__name__}: {e}"

    jobs.put(None)


def main():
    try:
        sh(["gh", "auth", "status"])
    except Exception:
        print("gh is not authenticated. Run `gh auth login` first.", file=sys.stderr)
        sys.exit(1)
    try:
        curses.wrapper(run)
    finally:
        try:
            print("\033[?1000l\033[?1006l\033[>4;0m", end="", flush=True)
        except Exception:
            pass
