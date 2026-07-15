"""State transitions and job orchestration (glue between UI and worker)."""
from .diff import compute_hunks
from .models import PR, FileEntry, PendingComment, FOCUS_DIFF
from .navigation import cur_file_path, hunk_anchor_line
from .tree import rebuild_tree
from .modals import show_editor_modal, show_review_modal

# Which "busy" spinner a job kind drives.
JOB_TAGS = {
    "load_prs": "prs",
    "load_active": "active",
    "checkout": "checkout",
    "mark_viewed": "viewed",
    "mark_viewed_bulk": "viewed",
    "load_pr_details": "details",
    "submit_review": "review",
    "add_pending": "pending",
    "discard_pending": "pending",
    "load_commit_diff": "commitdiff",
}


# ---------- mouse hit-testing ----------

def point_in(rect, my, mx):
    y, x, h, w = rect
    return y <= my < y + h and x <= mx < x + w


def pane_at(st, my, mx):
    for name, rect in st.rects.items():
        if point_in(rect, my, mx):
            return name
    return None


# ---------- jobs ----------

def submit_job(jobs, st, job):
    st.busy.add(JOB_TAGS[job[0]])
    jobs.put(job)


def _set_diff(st, diff, info):
    """Install a new diff and reset diff/hunk indexing over it."""
    st.diff_by_file = diff
    st.info_by_file = info
    st.hunks_by_file = {p: compute_hunks(lines) for p, lines in diff.items()}
    st.diff_scroll = 0
    st.diff_hunk_idx = 0


def apply_commit_selection(st, jobs):
    """Recompute the diff/files for the selected commit range.

    ``st.commits`` is newest-first, so the oldest commit in a range sits at the
    higher list index. Selection is treated as a contiguous range: everything
    between the earliest and latest selected commit is reviewed, so the checkbox
    set is normalised to fill any gap.
    """
    if not st.active_pr or not st.commits:
        return
    picked = [i for i, c in enumerate(st.commits) if c.oid in st.commit_selected]
    if not picked:
        st.status = "Select at least one commit to review."
        return
    lo, hi = picked[0], picked[-1]  # lo = newest end, hi = oldest end
    # Normalise to a contiguous range so the checkboxes match the reviewed diff.
    st.commit_selected = {st.commits[i].oid for i in range(lo, hi + 1)}
    if "commitdiff" in st.busy:
        return
    oldest, newest = st.commits[hi], st.commits[lo]
    submit_job(jobs, st, ("load_commit_diff", oldest.oid, newest.oid))
    n = hi - lo + 1
    st.status = (f"Reviewing {n} commit{'s' if n != 1 else ''} "
                 f"({oldest.short}..{newest.short})…")


def maybe_load_details(st, jobs):
    if not st.prs or "details" in st.busy:
        return
    pr = st.prs[st.pr_idx]
    if pr.number not in st.pr_details:
        st.pr_details[pr.number] = None
        submit_job(jobs, st, ("load_pr_details", pr.number))


def apply_result(st, res, jobs):
    """Fold a worker result tuple into ``State``."""
    kind = res[0]
    if kind == "prs":
        st.prs = res[1]
        st.busy.discard("prs")
        if st.pr_idx >= len(st.prs):
            st.pr_idx = max(0, len(st.prs) - 1)
    elif kind == "active":
        st.busy.discard("active")
        number, pr_id, files, diff, info, pending, commits = res[1:8]
        st.pending = pending
        if st.pending_idx >= len(st.pending):
            st.pending_idx = max(0, len(st.pending) - 1)
        if number is None:
            st.active_pr = None
            st.files = []
            st.viewed_by_path = {}
            st.commits = []
            st.commit_selected = set()
            st.commit_idx = 0
            st.commit_view_offset = 0
            st.diff_by_file = {}
            st.info_by_file = {}
            st.hunks_by_file = {}
            st.diff_scroll = 0
            st.diff_hunk_idx = 0
        else:
            match = next((p for p in st.prs if p.number == number), None)
            if match is None:
                match = PR(number=number, title=f"#{number}", head="", author="", node_id=pr_id)
            else:
                match.node_id = pr_id
            st.active_pr = match
            st.files = files
            st.viewed_by_path = {f.path: f.viewed for f in files}
            st.commits = commits
            st.commit_selected = {c.oid for c in commits}  # all selected by default
            st.commit_idx = 0
            st.commit_view_offset = 0
            _set_diff(st, diff, info)
        st.file_idx = 0
        st.file_view_offset = 0
        rebuild_tree(st)
    elif kind == "commit_diff":
        _, diff, info = res
        st.busy.discard("commitdiff")
        _set_diff(st, diff, info)
        # Filter the file tree to the files touched by the selected range,
        # keeping viewed state from the PR-wide view.
        paths = sorted(diff.keys())
        st.files = [FileEntry(p, st.viewed_by_path.get(p, False)) for p in paths]
        st.file_idx = 0
        st.file_view_offset = 0
        rebuild_tree(st)
        n = len(st.files)
        st.status = f"Reviewing {n} file{'s' if n != 1 else ''} in selected commits"
    elif kind == "checkout_done":
        st.busy.discard("checkout")
        st.status = f"Checked out #{res[1]} — reloading files…"
        submit_job(jobs, st, ("load_active", st.repo_owner, st.repo_name, st.viewer))
    elif kind == "viewed_ok":
        paths, viewed = res[1], res[2]
        pset = set(paths)
        for f in st.files:
            if f.path in pset:
                f.viewed = viewed
        for p in pset:
            st.viewed_by_path[p] = viewed
        st.busy.discard("viewed")
        st.status = f"{'Marked' if viewed else 'Unmarked'} {len(paths)} file{'s' if len(paths) != 1 else ''}"
    elif kind == "viewed_bulk":
        done, viewed, errs = res[1], res[2], res[3]
        dset = set(done)
        for f in st.files:
            if f.path in dset:
                f.viewed = viewed
        for p in dset:
            st.viewed_by_path[p] = viewed
        st.busy.discard("viewed")
        st.status = f"{'Marked' if viewed else 'Unmarked'} {len(done)}, {len(errs)} failed"
    elif kind == "pr_details":
        number, data = res[1], res[2]
        st.pr_details[number] = data
        st.busy.discard("details")
    elif kind == "pending_list":
        st.pending = res[1]
        if st.pending_idx >= len(st.pending):
            st.pending_idx = max(0, len(st.pending) - 1)
        st.busy.discard("pending")
        st.status = res[2] if len(res) > 2 else "Pending review updated"
    elif kind == "review_submitted":
        st.pending = []
        st.pending_idx = 0
        st.busy.discard("review")
        st.status = f"Review submitted ({res[1]})"
    elif kind == "error":
        _, job_kind, msg = res
        st.busy.discard(JOB_TAGS.get(job_kind, ""))
        st.status = f"[{job_kind}] {msg}"


# ---------- modal-driven actions ----------

def open_comment_modal(stdscr, st, jobs):
    if st.focus != FOCUS_DIFF or not st.active_pr:
        st.status = "Focus the diff pane to comment on a hunk."
        return
    path = cur_file_path(st)
    if not path:
        return
    anchor = hunk_anchor_line(st, path)
    if anchor is None:
        st.status = "No commentable line in the current hunk."
        return
    line, side = anchor
    action, body = show_editor_modal(
        stdscr,
        f"Comment on {path}:{line} ({side})",
        "Enter: add to pending review · Shift+Enter: newline · Esc: cancel",
    )
    if action == "cancel" or not body.strip():
        return
    # Any confirm adds to the pending review, which is created/persisted
    # server-side so it survives restarts.
    comment = PendingComment(path=path, body=body.strip(), line=line, side=side)
    st.pending.append(comment)  # optimistic; the reload replaces the list
    submit_job(jobs, st, ("add_pending", st.repo_owner, st.repo_name,
                          st.active_pr.number, st.viewer, st.active_pr.node_id, comment))
    st.status = f"Adding comment on {path}:{line} to pending review…"


def open_finish_modal(stdscr, st, jobs):
    if not st.active_pr:
        st.status = "No active PR."
        return
    result = show_review_modal(stdscr, len(st.pending))
    if not result:
        return
    event, body = result
    submit_job(jobs, st, ("submit_review", st.repo_owner, st.repo_name,
                          st.active_pr.number, st.viewer, st.active_pr.node_id, event, body))
    st.status = f"Submitting review ({event})…"
