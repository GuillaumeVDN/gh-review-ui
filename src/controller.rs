//! State transitions and job orchestration (glue between UI and worker).

use std::sync::mpsc::Sender;

use crate::api;
use crate::diff::compute_hunks;
use crate::editor;
use crate::models::{
    Category, ConfirmKind, FileEntry, Focus, Overlay, PendingComment, Pr, StageState, State,
    TreeRow, SUBMIT_CHOICES,
};
use crate::navigation::{
    cur_file_path, current_hunk_range, diff_path, first_change_index, hunk_comment_indices,
    hunk_unstages, is_local_diff, is_split, line_target, stage_state,
};
use crate::textbuffer::TextArea;
use crate::tree;
use crate::worker::{job_tag, Job, Msg};

pub fn submit(st: &mut State, tx: &Sender<Job>, job: Job) {
    st.busy.insert(job_tag(&job).to_string());
    let _ = tx.send(job);
}

fn set_diff(
    st: &mut State,
    diff: std::collections::HashMap<String, Vec<String>>,
    info: std::collections::HashMap<String, Vec<crate::models::LineInfo>>,
) {
    st.hunks_by_file = diff.iter().map(|(p, l)| (p.clone(), compute_hunks(l))).collect();
    st.diff_by_file = diff;
    st.info_by_file = info;
    st.diff_scroll = 0;
    st.diff_hunk_idx = 0;
    st.diff_reveal_pending = true;
    st.comment_mode = false;
    st.comment_start = None;
    st.last_comment = None; // line indices don't survive a new diff
}

/// Wipe the previous PR's panels so they read "Loading…", and focus Files.
fn reset_review_panels(st: &mut State) {
    st.files.clear();
    st.tree.clear();
    st.file_idx = 0;
    st.file_offset = 0;
    st.commits.clear();
    st.commit_selected.clear();
    st.commit_idx = 0;
    st.commit_offset = 0;
    st.pending.clear();
    st.pending_idx = 0;
    st.pending_offset = 0;
    st.edit_files.clear();
    st.edit_tree.clear();
    st.edit_diff_by_file.clear();
    st.edit_info_by_file.clear();
    st.unstaged_diff_by_file.clear();
    st.staged_diff_by_file.clear();
    st.edit_idx = 0;
    st.edit_offset = 0;
    st.staged_side = false;
    st.alt_diff_view = (0, 0);
    set_diff(st, Default::default(), Default::default());
    st.focus = Focus::Files;
}

/// Check out a PR in a worktree, focus Files, and kick off the open.
pub fn begin_open_pr(st: &mut State, tx: &Sender<Job>, pr: Pr) {
    reset_review_panels(st);
    st.local_mode = false;
    st.status = format!("Opening #{} in a worktree…", pr.number);
    let (repo_root, owner, name) = (st.repo_root.clone(), st.repo_owner.clone(), st.repo_name.clone());
    submit(st, tx, Job::OpenPr { repo_root, owner, name, number: pr.number, head: pr.head.clone() });
}

/// Open the locally checked-out PR in place (main repo, no worktree).
pub fn begin_open_local_pr(st: &mut State, tx: &Sender<Job>, pr: Pr) {
    reset_review_panels(st);
    st.local_mode = true;
    st.active_worktree = st.repo_root.clone();
    st.active_pr = Some(pr.clone());
    st.status = format!("Reviewing #{} locally…", pr.number);
    let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
    submit(st, tx, Job::LoadActive { owner, name, login, number: Some(pr.number), local: true });
}

pub fn maybe_load_details(st: &mut State, tx: &Sender<Job>) {
    if st.prs.is_empty() || st.busy.contains("details") {
        return;
    }
    let n = st.prs[st.pr_idx.min(st.prs.len() - 1)].number;
    if !st.pr_details.contains_key(&n) {
        st.pr_details.insert(n, None);
        submit(st, tx, Job::LoadPrDetails(n));
    }
}

/// Recompute the diff/files for the selected commit range (contiguous).
pub fn apply_commit_selection(st: &mut State, tx: &Sender<Job>) {
    if st.active_pr.is_none() || st.commits.is_empty() {
        return;
    }
    let picked: Vec<usize> = st
        .commits
        .iter()
        .enumerate()
        .filter(|(_, c)| st.commit_selected.contains(&c.oid))
        .map(|(i, _)| i)
        .collect();
    if picked.is_empty() {
        st.status = "Select at least one commit to review.".into();
        return;
    }
    let (lo, hi) = (picked[0], picked[picked.len() - 1]);
    st.commit_selected = (lo..=hi).map(|i| st.commits[i].oid.clone()).collect();
    if st.busy.contains("commitdiff") {
        return;
    }
    let (oldest, newest) = (st.commits[hi].clone(), st.commits[lo].clone());
    let n = hi - lo + 1;
    st.status = format!(
        "Reviewing {n} commit{} ({}..{})…",
        if n == 1 { "" } else { "s" },
        oldest.short(),
        newest.short()
    );
    submit(st, tx, Job::LoadCommitDiff { first: oldest.oid, last: newest.oid });
}

pub fn apply_msg(st: &mut State, msg: Msg, tx: &Sender<Job>) {
    match msg {
        Msg::Prs(prs) => {
            st.prs = prs;
            st.busy.remove("prs");
            if st.pr_idx >= st.prs.len() {
                st.pr_idx = st.prs.len().saturating_sub(1);
            }
            // Backfill the active PR's head branch (needed to push edits) when it
            // was opened before the list — e.g. reopened from last session.
            let head = st
                .active_pr
                .as_ref()
                .filter(|a| a.head.is_empty())
                .and_then(|a| st.prs.iter().find(|p| p.number == a.number).map(|p| p.head.clone()));
            if let (Some(h), Some(a)) = (head, st.active_pr.as_mut()) {
                a.head = h;
            }
        }
        Msg::Active { number, pr_id, files, diff, info, pending, commits } => {
            st.busy.remove("active");
            st.pending = pending;
            if st.pending_idx >= st.pending.len() {
                st.pending_idx = st.pending.len().saturating_sub(1);
            }
            match number {
                None => {
                    st.active_pr = None;
                    st.active_worktree.clear();
                    st.files.clear();
                    st.pr_files.clear();
                    st.viewed_by_path.clear();
                    st.commits.clear();
                    st.commit_selected.clear();
                    st.commit_idx = 0;
                    st.commit_offset = 0;
                    set_diff(st, Default::default(), Default::default());
                }
                Some(n) => {
                    // Opening a *different* PR expands the whole tree so every
                    // file shows — even ones already viewed/folded last session.
                    if st.active_pr.as_ref().map_or(true, |p| p.number != n) {
                        st.collapsed_dirs.clear();
                    }
                    let matched = st.prs.iter().find(|p| p.number == n).cloned();
                    let mut pr = matched.unwrap_or(Pr {
                        number: n,
                        title: format!("#{n}"),
                        head: String::new(),
                        author: String::new(),
                        node_id: pr_id.clone(),
                        category: Category::Review,
                        created_at: String::new(),
                        updated_at: String::new(),
                    });
                    pr.node_id = pr_id;
                    st.active_pr = Some(pr);
                    st.viewed_by_path = files.iter().map(|f| (f.path.clone(), f.viewed)).collect();
                    st.pr_files = files.clone();
                    st.files = files;
                    st.commit_selected = commits.iter().map(|c| c.oid.clone()).collect();
                    st.commits = commits;
                    st.commit_idx = 0;
                    st.commit_offset = 0;
                    set_diff(st, diff, info);
                    st.status = format!(
                        "#{n} · {} file{} · {} commit{}",
                        st.files.len(),
                        if st.files.len() == 1 { "" } else { "s" },
                        st.commits.len(),
                        if st.commits.len() == 1 { "" } else { "s" },
                    );
                }
            }
            st.file_idx = 0;
            st.file_offset = 0;
            tree::rebuild(st);
            reload_edits(st, tx);
        }
        Msg::CommitDiff { diff, info } => {
            st.busy.remove("commitdiff");
            let mut paths: Vec<String> = diff.keys().cloned().collect();
            paths.sort();
            st.files = paths
                .iter()
                .map(|p| FileEntry { path: p.clone(), viewed: *st.viewed_by_path.get(p).unwrap_or(&false) })
                .collect();
            st.pr_files = st.files.clone();
            set_diff(st, diff, info);
            st.file_idx = 0;
            st.file_offset = 0;
            tree::rebuild(st);
            st.status = format!("Reviewing {} file{} in selected commits", st.files.len(), if st.files.len() == 1 { "" } else { "s" });
        }
        Msg::PrOpened { number, path } => {
            st.active_worktree = path;
            st.busy.remove("worktree");
            st.status = format!("Worktree ready for #{number} — loading…");
            api::save_last_pr(&st.repo_owner, &st.repo_name, number);
            let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
            submit(st, tx, Job::LoadActive { owner, name, login, number: Some(number), local: false });
        }
        Msg::ViewedOk { paths, viewed } => {
            // Optimistic state already matches; just confirm and clear the record.
            for f in st.files.iter_mut() {
                if paths.contains(&f.path) {
                    f.viewed = viewed;
                }
            }
            for p in &paths {
                st.viewed_by_path.insert(p.clone(), viewed);
            }
            st.viewed_inflight = None;
            st.busy.remove("viewed");
            st.status = format!("{} {} file{}", if viewed { "Marked" } else { "Unmarked" }, paths.len(), if paths.len() == 1 { "" } else { "s" });
        }
        Msg::ViewedBulk { done, viewed, errs } => {
            for f in st.files.iter_mut() {
                if done.contains(&f.path) {
                    f.viewed = viewed;
                }
            }
            for p in &done {
                st.viewed_by_path.insert(p.clone(), viewed);
            }
            // Revert any optimistically-marked path that didn't actually succeed.
            if let Some((paths, target)) = st.viewed_inflight.take() {
                for f in st.files.iter_mut() {
                    if paths.contains(&f.path) && !done.contains(&f.path) {
                        f.viewed = !target;
                    }
                }
                for p in &paths {
                    if !done.contains(p) {
                        st.viewed_by_path.insert(p.clone(), !target);
                    }
                }
            }
            st.busy.remove("viewed");
            st.status = format!("{} {}, {} failed", if viewed { "Marked" } else { "Unmarked" }, done.len(), errs);
        }
        Msg::PrDetails { number, data } => {
            st.pr_details.insert(number, Some(data));
            st.busy.remove("details");
        }
        Msg::PendingList { pending, status } => {
            st.pending = pending;
            if st.pending_idx >= st.pending.len() {
                st.pending_idx = st.pending.len().saturating_sub(1);
            }
            st.busy.remove("pending");
            st.status = status;
        }
        Msg::ReviewSubmitted(event) => {
            st.pending.clear();
            st.pending_idx = 0;
            st.busy.remove("review");
            st.status = format!("Review submitted ({event})");
        }
        Msg::Edits(edits) => {
            st.busy.remove("edits");
            let api::Edits { files, combined, unstaged, staged } = edits;
            st.edit_hunks_by_file = combined.0.iter().map(|(p, l)| (p.clone(), compute_hunks(l))).collect();
            st.unstaged_hunks_by_file = unstaged.0.iter().map(|(p, l)| (p.clone(), compute_hunks(l))).collect();
            st.staged_hunks_by_file = staged.0.iter().map(|(p, l)| (p.clone(), compute_hunks(l))).collect();
            st.edit_kind_by_path = files.iter().map(|e| (e.path.clone(), e.kind)).collect();
            st.edit_files = files;
            (st.edit_diff_by_file, st.edit_info_by_file) = combined;
            (st.unstaged_diff_by_file, st.unstaged_info_by_file) = unstaged;
            (st.staged_diff_by_file, st.staged_info_by_file) = staged;
            st.edit_diff_scroll = 0;
            tree::rebuild_edits(st);
            if st.edit_idx >= st.edit_tree.len() {
                st.edit_idx = st.edit_tree.len().saturating_sub(1);
            }
            // A file whose local diff is on screen may have just been committed away.
            if let Some(p) = st.local_diff_path.clone() {
                if !st.edit_diff_by_file.contains_key(&p) {
                    st.local_diff_path = None;
                    if st.focus == Focus::Diff {
                        st.focus = Focus::Edits;
                    }
                } else if !is_split(st, &p) {
                    // Staging closed the split: back to the single combined view.
                    st.staged_side = false;
                    st.alt_diff_view = (0, 0);
                }
            }
            // Merge edit-only files (new/deleted/renamed, not in the PR diff) into
            // the Files tree so [3] shows them too (item 7).
            merge_edit_files_into_tree(st);
        }
        Msg::EditsCommitted { status } => {
            st.busy.remove("editcommit");
            st.status = status;
            reload_edits(st, tx); // now clean
        }
        Msg::Done { kind, msg } => {
            st.busy.remove(&kind);
            st.status = msg;
            if kind == "editpush" {
                reload_edits(st, tx); // committed edits are gone from the worktree
            }
        }
        Msg::Error { kind, msg } => {
            st.busy.remove(&kind);
            // Roll back an optimistic viewed change that failed on the server.
            if kind == "viewed" {
                if let Some((paths, target)) = st.viewed_inflight.take() {
                    for f in st.files.iter_mut() {
                        if paths.contains(&f.path) {
                            f.viewed = !target;
                        }
                    }
                    for p in &paths {
                        st.viewed_by_path.insert(p.clone(), !target);
                    }
                }
            }
            st.status = format!("[{kind}] {msg}");
        }
    }
}

// ---- ask Claude about a hunk ----

/// Open the "ask Claude" modal for the currently selected hunk.
pub fn begin_ask(st: &mut State) {
    if diff_path(st).is_none() {
        st.status = "No file/hunk selected.".into();
        return;
    }
    st.overlay = Overlay::Ask { ta: TextArea::new("") };
}

/// Build a prompt from the selected hunk + question and launch Claude in a new
/// Ghostty window.
pub fn confirm_ask(st: &mut State) {
    let Overlay::Ask { ta } = &st.overlay else { return };
    let question = ta.text().trim().to_string();
    let path = diff_path(st);

    // A few context lines of diff around the selected change block (local diff if
    // that's what's on screen, else the PR diff).
    let mut snippet = String::new();
    if let Some(p) = &path {
        let src = if is_local_diff(st, p) { &st.edit_diff_by_file } else { &st.diff_by_file };
        if let (Some(lines), Some((s, e))) = (src.get(p), current_hunk_range(st, p)) {
            let lo = s.saturating_sub(6);
            let hi = (e + 6).min(lines.len());
            for l in &lines[lo..hi] {
                snippet.push_str(l);
                snippet.push('\n');
            }
        }
    }
    st.overlay = Overlay::None;
    if question.is_empty() {
        st.status = "Ask cancelled (empty question).".into();
        return;
    }

    let (num, title) = st
        .active_pr
        .as_ref()
        .map(|p| (p.number, p.title.clone()))
        .unwrap_or((0, String::new()));
    let file = path.as_deref().unwrap_or("?");
    let prompt = format!(
        "I'm reviewing GitHub PR #{num} ({title}) and have a question about a change.\n\n\
         File: {file}\n\
         You're running in the PR's checked-out worktree, so you can open that file for full context.\n\n\
         The relevant diff hunk:\n\
         ```diff\n{snippet}```\n\n\
         My question:\n{question}\n"
    );
    let cwd = if st.active_worktree.is_empty() { st.repo_root.clone() } else { st.active_worktree.clone() };
    match editor::ask_claude(&prompt, &cwd) {
        Ok(()) => st.status = "Launched Claude in a new terminal…".into(),
        Err(e) => st.status = format!("Failed to launch Claude: {e}"),
    }
}

// ---- comment line picker ----

/// From the picker, confirm on the current line: edit the comment anchored on
/// that exact line if there is one, otherwise start a new comment.
pub fn begin_comment_or_edit(st: &mut State) {
    if let Some(path) = cur_file_path(st) {
        if let Some(idx) = comment_at_diff_index(st, &path, st.comment_line) {
            st.comment_mode = false;
            st.pending_idx = idx;
            begin_edit_pending(st);
            return;
        }
    }
    begin_comment(st);
}

/// Index into `st.pending` of a comment anchored exactly on diff-line `idx`.
fn comment_at_diff_index(st: &State, path: &str, idx: usize) -> Option<usize> {
    let (old, new) = crate::navigation::info_lines(st, path)?.get(idx).copied()?;
    st.pending.iter().position(|c| {
        c.path == path && if c.side == "LEFT" { old == Some(c.line) } else { new == Some(c.line) }
    })
}

pub fn enter_comment_mode(st: &mut State) {
    if st.active_pr.is_none() {
        st.status = "No active PR.".into();
        return;
    }
    let Some(path) = diff_path(st) else { return };
    let idxs = hunk_comment_indices(st, &path);
    if idxs.is_empty() {
        st.status = "No commentable line in the current hunk.".into();
        return;
    }
    st.comment_mode = true;
    st.comment_start = None;
    // Keep any saved draft for this file — commenting it again restores it.
    st.diff_reveal_pending = true;
    // If the last comment was in the *currently selected* hunk, continue on the
    // next commentable line of that hunk; otherwise start at the hunk's first line.
    let same_hunk_next = match (&st.last_comment, current_hunk_range(st, &path)) {
        (Some((p, idx)), Some((s, e))) if *p == path && s <= *idx && *idx < e => {
            idxs.iter().copied().find(|&i| i > *idx)
        }
        _ => None,
    };
    st.comment_line = same_hunk_next
        .or_else(|| first_change_index(st, &path))
        .unwrap_or(idxs[0]);
    st.status = "Comment: j/k line · Shift+J/K range · Enter: comment/edit · Esc cancel".into();
}

pub fn move_comment(st: &mut State, direction: i64, extend: bool) {
    let Some(path) = diff_path(st) else { return };
    let idxs = hunk_comment_indices(st, &path);
    if idxs.is_empty() {
        return;
    }
    let mut pos = idxs.iter().position(|&i| i == st.comment_line).unwrap_or(0);
    pos = ((pos as i64 + direction).clamp(0, idxs.len() as i64 - 1)) as usize;
    if extend {
        if st.comment_start.is_none() {
            st.comment_start = Some(st.comment_line);
        }
    } else {
        st.comment_start = None;
    }
    st.comment_line = idxs[pos];
    st.diff_reveal_pending = true;
}

/// From the picker, open the comment editor overlay on the selected line/range.
pub fn begin_comment(st: &mut State) {
    st.comment_mode = false;
    let Some(path) = diff_path(st) else { return };
    let Some((mut line, mut side)) = line_target(st, &path, st.comment_line) else {
        st.status = "No commentable line selected.".into();
        return;
    };
    let (mut start_line, mut start_side) = (None, String::new());
    if let Some(anchor) = st.comment_start {
        if anchor != st.comment_line {
            let (lo, hi) = (anchor.min(st.comment_line), anchor.max(st.comment_line));
            if let (Some(lt), Some(ht)) = (line_target(st, &path, lo), line_target(st, &path, hi)) {
                start_line = Some(lt.0);
                start_side = lt.1;
                line = ht.0;
                side = ht.1;
            }
        }
    }
    // Restore this file's saved draft so re-opening keeps what was written.
    let draft = st.comment_drafts.get(&path).cloned().unwrap_or_default();
    let ta = TextArea::new(&draft);
    st.overlay = Overlay::Comment { ta, path, line, side, start_line, start_side };
}

fn stash_draft(st: &mut State, path: &str, text: String) {
    if text.trim().is_empty() {
        st.comment_drafts.remove(path);
    } else {
        st.comment_drafts.insert(path.to_string(), text);
    }
}

fn strip_diff_prefix(l: &str) -> &str {
    if l.starts_with(['+', '-', ' ']) {
        &l[1..]
    } else {
        l
    }
}

/// New-side (added/context) code for the diff-line index range `lo..=hi`.
/// Only lines present on the new side are kept — that's what a suggestion replaces.
fn code_by_diff_idx(st: &State, path: &str, lo: usize, hi: usize) -> Vec<String> {
    let (Some(lines), Some(info)) = (st.diff_by_file.get(path), st.info_by_file.get(path)) else {
        return Vec::new();
    };
    (lo..=hi)
        .filter_map(|i| match (lines.get(i), info.get(i)) {
            (Some(l), Some(&(_, Some(_)))) => Some(strip_diff_prefix(l).to_string()),
            _ => None,
        })
        .collect()
}

/// New-side code whose new-side line number falls in `[lo_line, hi_line]`.
fn code_by_new_line(st: &State, path: &str, lo_line: i64, hi_line: i64) -> Vec<String> {
    let (Some(lines), Some(info)) = (st.diff_by_file.get(path), st.info_by_file.get(path)) else {
        return Vec::new();
    };
    info.iter()
        .enumerate()
        .filter_map(|(i, &(_, new))| {
            let n = new?;
            (n >= lo_line && n <= hi_line)
                .then(|| lines.get(i).map(|l| strip_diff_prefix(l).to_string()))
                .flatten()
        })
        .collect()
}

/// Insert a GitHub ```suggestion block prefilled with the commented code (the
/// new-side content of the target line/range) into the open editor. Works both
/// for a new comment (`Overlay::Comment`) and an edited one (`Overlay::Edit`).
pub fn insert_suggestion(st: &mut State) {
    let code = match &st.overlay {
        Overlay::Comment { path, .. } => {
            let path = path.clone();
            let anchor = st.comment_start.unwrap_or(st.comment_line);
            let (lo, hi) = (anchor.min(st.comment_line), anchor.max(st.comment_line));
            code_by_diff_idx(st, &path, lo, hi)
        }
        Overlay::Edit { path, line, comment_id, .. } => {
            let path = path.clone();
            let hi_line = *line;
            // Recover the range's start line from the matching pending comment.
            let lo_line = st
                .pending
                .iter()
                .find(|c| &c.comment_id == comment_id)
                .and_then(|c| c.start_line)
                .unwrap_or(hi_line);
            code_by_new_line(st, &path, lo_line.min(hi_line), lo_line.max(hi_line))
        }
        _ => return,
    };
    let block = format!("```suggestion\n{}\n```", code.join("\n"));
    if let Overlay::Comment { ta, .. } | Overlay::Edit { ta, .. } = &mut st.overlay {
        ta.insert_str(&block);
    }
}

/// Close the comment editor back to the line picker, saving the draft (per file)
/// and keeping the current selection.
pub fn comment_to_picker(st: &mut State) {
    let overlay = std::mem::replace(&mut st.overlay, Overlay::None);
    if let Overlay::Comment { ta, path, .. } = overlay {
        stash_draft(st, &path, ta.text());
        st.comment_mode = true; // resume selection with the same line/range
        st.diff_reveal_pending = true;
        st.status = "Line selection — Enter: resume editing · j/k/Shift+J/K: adjust · Esc: cancel".into();
    }
}

pub fn confirm_comment(st: &mut State, tx: &Sender<Job>) {
    let overlay = std::mem::replace(&mut st.overlay, Overlay::None);
    let Overlay::Comment { ta, path, line, side, start_line, start_side } = overlay else {
        return;
    };
    let body = ta.text().trim().to_string();
    if body.is_empty() {
        return;
    }
    st.comment_drafts.remove(&path); // submitted — drop this file's draft
    // Remember the end line so the next `c` on this file continues below it.
    let end_idx = st.comment_start.map_or(st.comment_line, |s| s.max(st.comment_line));
    st.last_comment = Some((path.clone(), end_idx));
    let Some(pr) = st.active_pr.clone() else { return };
    if st.local_mode {
        // Store locally, never touch the PR.
        let comment = PendingComment {
            path: path.clone(),
            body,
            line,
            side,
            comment_id: local_comment_id(),
            start_line,
            start_side,
        };
        st.pending.push(comment);
        save_local(st);
        st.status = format!("Saved comment on {path}:{line} locally");
        return;
    }
    let comment = PendingComment {
        path: path.clone(),
        body,
        line,
        side,
        comment_id: String::new(),
        start_line,
        start_side,
    };
    st.pending.push(comment.clone()); // optimistic; the reload replaces it
    let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
    submit(st, tx, Job::AddPending { owner, name, number: pr.number, login, pr_id: pr.node_id, comment });
    st.status = format!("Adding comment on {path}:{line} to pending review…");
}

/// A unique id for a locally-stored comment.
fn local_comment_id() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("local-{n}")
}

/// Persist the current pending comments to the local store (local PR mode).
fn save_local(st: &State) {
    if let Some(pr) = &st.active_pr {
        api::save_local_comments(&st.repo_owner, &st.repo_name, pr.number, &st.pending);
    }
}

// ---- edit pending ----

pub fn begin_edit_pending(st: &mut State) {
    if st.pending_idx >= st.pending.len() {
        return;
    }
    let c = st.pending[st.pending_idx].clone();
    if c.comment_id.is_empty() {
        st.status = "Comment not saved on GitHub yet — try again in a moment.".into();
        return;
    }
    st.overlay = Overlay::Edit {
        ta: TextArea::new(&c.body),
        comment_id: c.comment_id,
        path: c.path,
        line: c.line,
    };
}

pub fn confirm_edit(st: &mut State, tx: &Sender<Job>) {
    let overlay = std::mem::replace(&mut st.overlay, Overlay::None);
    let Overlay::Edit { ta, comment_id, path, line } = overlay else { return };
    let body = ta.text().trim().to_string();
    if body.is_empty() {
        return;
    }
    let Some(pr) = st.active_pr.clone() else { return };
    for c in st.pending.iter_mut() {
        if c.comment_id == comment_id {
            c.body = body.clone();
        }
    }
    if st.local_mode {
        save_local(st);
        st.status = format!("Updated comment on {path}:{line} locally");
        return;
    }
    let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
    submit(st, tx, Job::EditPending { owner, name, number: pr.number, login, comment_id, body });
    st.status = format!("Updating comment on {path}:{line}…");
}

/// Discard the selected pending comment (the `d` key in [5]).
pub fn discard_selected_comment(st: &mut State, tx: &Sender<Job>) {
    if st.pending_idx >= st.pending.len() || st.busy.contains("pending") || st.active_pr.is_none() {
        return;
    }
    let removed = st.pending.remove(st.pending_idx);
    st.pending_idx = st.pending_idx.min(st.pending.len().saturating_sub(1));
    if st.local_mode {
        save_local(st);
        st.status = format!("Deleted comment on {}:{} locally", removed.path, removed.line);
        return;
    }
    let pr = st.active_pr.clone().unwrap();
    let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
    submit(
        st,
        tx,
        Job::DiscardPending { owner, name, number: pr.number, login, comment_id: removed.comment_id },
    );
    st.status = format!("Discarding comment on {}:{}…", removed.path, removed.line);
}

/// Delete the comment currently open in the Edit modal (Ctrl+D).
pub fn delete_editing_comment(st: &mut State, tx: &Sender<Job>) {
    let Overlay::Edit { comment_id, path, line, .. } = &st.overlay else { return };
    let (comment_id, path, line) = (comment_id.clone(), path.clone(), *line);
    st.overlay = Overlay::None;
    if let Some(pos) = st.pending.iter().position(|c| c.comment_id == comment_id) {
        st.pending.remove(pos);
        if st.pending_idx >= st.pending.len() {
            st.pending_idx = st.pending.len().saturating_sub(1);
        }
    }
    if st.local_mode {
        save_local(st);
        st.status = format!("Deleted comment on {path}:{line} locally");
        return;
    }
    if comment_id.is_empty() {
        return;
    }
    let Some(pr) = st.active_pr.clone() else { return };
    let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
    submit(st, tx, Job::DiscardPending { owner, name, number: pr.number, login, comment_id });
    st.status = format!("Deleting comment on {path}:{line}…");
}

// ---- finish review ----

pub fn begin_review(st: &mut State) {
    if st.active_pr.is_none() {
        st.status = "No active PR.".into();
        return;
    }
    st.overlay = Overlay::Review { ta: TextArea::new(""), editing: true, choice: 0 };
}

pub fn confirm_review(st: &mut State, tx: &Sender<Job>) {
    let overlay = std::mem::replace(&mut st.overlay, Overlay::None);
    let Overlay::Review { ta, choice, .. } = overlay else { return };
    let Some(pr) = st.active_pr.clone() else { return };
    let event = SUBMIT_CHOICES[choice.min(SUBMIT_CHOICES.len() - 1)].0.to_string();
    let body = ta.text();

    if event == "CLAUDE" {
        if st.pending.is_empty() && body.trim().is_empty() {
            st.status = "No comments to send.".into();
            return;
        }
        let prompt = build_review_prompt(st, &body);
        match editor::send_review_to_claude(st, &prompt) {
            Ok(()) => st.status = "Sent review comments to Claude…".into(),
            Err(e) => st.status = format!("Failed to launch Claude: {e}"),
        }
        return;
    }

    let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
    if st.local_mode {
        // Comments live locally — post them all to the PR, then submit the review.
        let comments = st.pending.clone();
        st.status = format!("Posting {} comment(s) and submitting review ({event})…", comments.len());
        submit(st, tx, Job::PostLocalReview {
            owner, name, number: pr.number, login, pr_id: pr.node_id, comments, event, body,
        });
        return;
    }
    st.status = format!("Submitting review ({event})…");
    submit(st, tx, Job::SubmitReview { owner, name, number: pr.number, login, pr_id: pr.node_id, event, body });
}

/// Assemble a prompt handing all pending review comments (with diff context) to
/// Claude to address in the checkout.
fn build_review_prompt(st: &State, body: &str) -> String {
    let (num, title) = st
        .active_pr
        .as_ref()
        .map(|p| (p.number, p.title.clone()))
        .unwrap_or((0, String::new()));
    let mut s = format!(
        "I'm reviewing GitHub PR #{num} ({title}). You're in the checkout for it. \
         Please address these review comments by editing the code:\n\n"
    );
    if !body.trim().is_empty() {
        s.push_str(&format!("Overall note: {}\n\n", body.trim()));
    }
    for c in &st.pending {
        let loc = match c.start_line {
            Some(sl) => format!("{}-{}", sl.min(c.line), sl.max(c.line)),
            None => c.line.to_string(),
        };
        s.push_str(&format!("### {}:{loc}\n", c.path));
        let (hunk, _) = crate::navigation::hunk_for_comment(st, c);
        if !hunk.is_empty() {
            s.push_str("```diff\n");
            for l in &hunk {
                s.push_str(l);
                s.push('\n');
            }
            s.push_str("```\n");
        }
        s.push_str(&format!("Comment: {}\n\n", c.body));
    }
    s
}

// ---- files pane helpers ----

pub fn toggle_collapse(st: &mut State, path: &str) {
    if st.collapsed_dirs.contains(path) {
        st.collapsed_dirs.remove(path);
    } else {
        st.collapsed_dirs.insert(path.to_string());
    }
    tree::rebuild(st);
}

pub fn mark_viewed(st: &mut State, tx: &Sender<Job>) {
    if st.file_idx >= st.tree.len() || st.active_pr.is_none() || st.busy.contains("viewed") {
        return;
    }
    let pr_id = st.active_pr.as_ref().unwrap().node_id.clone();
    match st.tree[st.file_idx].clone() {
        crate::models::TreeRow::File { index, .. } => {
            let path = st.files[index].path.clone();
            let new_v = !st.files[index].viewed;
            // Optimistic: reflect it now so a following `z`/navigation sees it.
            st.files[index].viewed = new_v;
            st.viewed_by_path.insert(path.clone(), new_v);
            st.viewed_inflight = Some((vec![path.clone()], new_v));
            st.status = format!("{} {path}…", if new_v { "Marking" } else { "Unmarking" });
            submit(st, tx, Job::MarkViewed { pr_id, path, viewed: new_v });
        }
        crate::models::TreeRow::Dir { path, .. } => {
            let idxs = tree::files_under_dir(st, &path);
            if idxs.is_empty() {
                return;
            }
            let all_v = idxs.iter().all(|&i| st.files[i].viewed);
            let new_v = !all_v;
            let paths: Vec<String> = idxs
                .iter()
                .filter(|&&i| st.files[i].viewed != new_v)
                .map(|&i| st.files[i].path.clone())
                .collect();
            if !paths.is_empty() {
                // Optimistic update for the whole batch.
                for f in st.files.iter_mut() {
                    if paths.contains(&f.path) {
                        f.viewed = new_v;
                    }
                }
                for p in &paths {
                    st.viewed_by_path.insert(p.clone(), new_v);
                }
                st.viewed_inflight = Some((paths.clone(), new_v));
                st.status = format!("{} {} files in {path}/…", if new_v { "Marking" } else { "Unmarking" }, paths.len());
                submit(st, tx, Job::MarkViewedBulk { pr_id, paths, viewed: new_v });
            }
        }
    }
}

pub fn fold_viewed(st: &mut State) {
    // Capture the cursor context before folding collapses rows away (so we keep
    // the underlying file index / dir path, not the volatile row position).
    let (dir, file_anchor) = match st.tree.get(st.file_idx) {
        Some(TreeRow::Dir { path, .. }) => (Some(path.clone()), None),
        Some(TreeRow::File { index, .. }) => (None, Some(*index)),
        None => (None, None),
    };
    let folded = tree::fold_viewed_dirs(st);
    let jumped = if let Some(path) = dir {
        // On a folder: dive to the first unviewed file inside it; only if it has
        // none (fully viewed → just folded) fall back to jumping onward from it.
        tree::first_unviewed_in_dir(st, &path).or_else(|| {
            let anchor = tree::files_under_dir(st, &path).into_iter().next();
            tree::next_unviewed_index(st, anchor)
        })
    } else {
        tree::next_unviewed_index(st, file_anchor)
    };
    if let Some(ti) = jumped {
        st.file_idx = ti;
        st.diff_scroll = 0;
        st.diff_hunk_idx = 0;
    }
    st.status = format!(
        "Folded {folded} viewed folder{}{}",
        if folded == 1 { "" } else { "s" },
        if jumped.is_some() { " · jumped to next unviewed file" } else { " · no unviewed files" }
    );
}

// ---- pending edits (local worktree changes) ----

/// Rebuild the Files list as the PR files plus edit-only local files (new /
/// deleted / renamed) so [3] shows them too (item 7).
fn merge_edit_files_into_tree(st: &mut State) {
    let mut files = st.pr_files.clone();
    for e in &st.edit_files {
        if !st.pr_files.iter().any(|f| f.path == e.path) {
            let viewed = *st.viewed_by_path.get(&e.path).unwrap_or(&false);
            files.push(FileEntry { path: e.path.clone(), viewed });
        }
    }
    st.files = files;
    tree::rebuild(st);
}

/// Paths of the pending edits under `dir` (its whole subtree).
fn edit_paths_under_dir(st: &State, dir: &str) -> Vec<String> {
    let prefix = format!("{dir}/");
    st.edit_files.iter().filter(|e| e.path.starts_with(&prefix)).map(|e| e.path.clone()).collect()
}

/// Stage / unstage the selected file (or whole folder) in [4] — the pending
/// edits' equivalent of marking a PR file viewed.
pub fn toggle_stage(st: &mut State, tx: &Sender<Job>) {
    if st.active_worktree.is_empty() || st.busy.contains("edits") {
        return;
    }
    let (paths, label) = match st.edit_tree.get(st.edit_idx).cloned() {
        Some(TreeRow::File { index, .. }) => match st.edit_files.get(index) {
            Some(e) => (vec![e.path.clone()], e.path.clone()),
            None => return,
        },
        Some(TreeRow::Dir { path, .. }) => (edit_paths_under_dir(st, &path), format!("{path}/")),
        None => return,
    };
    if paths.is_empty() {
        return;
    }
    // A folder (or a partly-staged file) stages first, and only unstages once
    // everything under it is staged.
    let unstage = paths.iter().all(|p| stage_state(st, p) == StageState::Staged);
    // Optimistic wording: a failure comes back as an Error and overwrites this.
    st.status = format!("{} {label}", if unstage { "Unstaged" } else { "Staged" });
    let wt = st.active_worktree.clone();
    submit(st, tx, Job::Stage { wt, paths, patch: None, unstage });
}

/// Stage / unstage the selected change block of the local diff shown in [0]
/// (lazygit-style hunk staging). On a split, the focused column decides the
/// direction; otherwise the file's own side does.
pub fn toggle_stage_hunk(st: &mut State, tx: &Sender<Job>) {
    if st.active_worktree.is_empty() || st.busy.contains("edits") {
        return;
    }
    let Some(path) = diff_path(st) else { return };
    if !is_local_diff(st, &path) {
        st.status = "Staging only applies to the local diff (Enter from [4]).".into();
        return;
    }
    let unstage = hunk_unstages(st, &path);
    let Some(block) = current_hunk_range(st, &path) else { return };
    let Some(lines) = crate::navigation::diff_lines(st, &path) else { return };
    let Some(patch) = crate::diff::build_hunk_patch(lines, block, unstage) else {
        st.status = "That block can't be staged by hunk.".into();
        return;
    };
    st.status = format!("{} a hunk of {path}", if unstage { "Unstaged" } else { "Staged" });
    let wt = st.active_worktree.clone();
    submit(st, tx, Job::Stage { wt, paths: vec![path], patch: Some(patch), unstage });
}

/// Move the cursor between the unstaged (left) and staged (right) columns of a
/// split local diff, each keeping its own scroll + selected block.
pub fn switch_stage_side(st: &mut State, staged: bool) {
    let Some(path) = diff_path(st) else { return };
    if !is_local_diff(st, &path) || !is_split(st, &path) || st.staged_side == staged {
        return;
    }
    st.staged_side = staged;
    let cur = (st.diff_scroll, st.diff_hunk_idx);
    (st.diff_scroll, st.diff_hunk_idx) = st.alt_diff_view;
    st.alt_diff_view = cur;
    st.comment_mode = false;
    st.diff_reveal_pending = true;
}

/// Show the selected pending-edit file's local diff in [0] with hunk navigation.
pub fn enter_local_diff(st: &mut State) {
    let Some(TreeRow::File { index, .. }) = st.edit_tree.get(st.edit_idx).cloned() else {
        return;
    };
    let Some(entry) = st.edit_files.get(index) else { return };
    let path = entry.path.clone();
    if !st.edit_diff_by_file.contains_key(&path) {
        return;
    }
    st.local_diff_path = Some(path);
    st.focus = Focus::Diff;
    st.diff_scroll = 0;
    st.diff_hunk_idx = 0;
    st.staged_side = false; // a split opens on the unstaged (left) column
    st.alt_diff_view = (0, 0);
    st.diff_reveal_pending = true;
}

/// Refresh the pending-edits list from the worktree (no-op without a worktree).
pub fn reload_edits(st: &mut State, tx: &Sender<Job>) {
    if st.active_worktree.is_empty() || st.busy.contains("edits") {
        return;
    }
    let wt = st.active_worktree.clone();
    submit(st, tx, Job::LoadEdits { wt });
}

/// Ask to revert the selected file's local change (confirmation before the
/// destructive discard).
pub fn begin_discard_edit(st: &mut State) {
    if st.busy.contains("edits") || st.active_worktree.is_empty() {
        return;
    }
    let Some(TreeRow::File { index, .. }) = st.edit_tree.get(st.edit_idx).cloned() else {
        return;
    };
    let Some(entry) = st.edit_files.get(index).cloned() else { return };
    let added = entry.kind == crate::models::EditKind::Added;
    let verb = if added { "Delete new file" } else { "Discard local changes to" };
    st.overlay = Overlay::Confirm {
        prompt: format!("{verb} {}?  (y/n)", entry.path),
        kind: ConfirmKind::RevertEdit { path: entry.path, added },
    };
}

/// Perform the confirmed action from an [`Overlay::Confirm`].
pub fn confirm_action(st: &mut State, tx: &Sender<Job>) {
    let overlay = std::mem::replace(&mut st.overlay, Overlay::None);
    let Overlay::Confirm { kind, .. } = overlay else { return };
    match kind {
        ConfirmKind::RevertEdit { path, added } => {
            if st.active_worktree.is_empty() {
                return;
            }
            let wt = st.active_worktree.clone();
            st.status = format!("Reverting local changes to {path}…");
            submit(st, tx, Job::DiscardEdit { wt, path, added });
        }
    }
}

/// Open the commit-message modal for the pending edits (`c` in [4]).
pub fn begin_commit_edits(st: &mut State) {
    if st.active_worktree.is_empty() {
        st.status = "No worktree — nothing to commit.".into();
        return;
    }
    if st.edit_files.is_empty() {
        st.status = "No local changes to commit.".into();
        return;
    }
    st.overlay = Overlay::CommitMsg { ta: TextArea::new("") };
}

/// Commit the pending edits with the entered message (no push).
pub fn confirm_commit_edits(st: &mut State, tx: &Sender<Job>) {
    let Overlay::CommitMsg { ta } = &st.overlay else { return };
    let message = ta.text().trim().to_string();
    if message.is_empty() {
        st.status = "Commit message is empty.".into();
        return;
    }
    if st.active_worktree.is_empty() || st.edit_files.is_empty() {
        return;
    }
    let paths: Vec<String> = st.edit_files.iter().map(|e| e.path.clone()).collect();
    st.overlay = Overlay::None;
    st.status = format!("Committing {} file(s)…", paths.len());
    submit(st, tx, Job::CommitEdits { wt: st.active_worktree.clone(), message, paths });
}

/// Push the committed edits to the PR branch (`P` in [4]); re-mark the pushed
/// files as viewed if they were already viewed (so they don't reappear as new).
pub fn push_edits(st: &mut State, tx: &Sender<Job>) {
    if st.busy.contains("editpush") {
        return;
    }
    let Some(pr) = st.active_pr.clone() else { return };
    if pr.head.is_empty() {
        st.status = "Unknown PR head branch — cannot push.".into();
        return;
    }
    if st.active_worktree.is_empty() {
        return;
    }
    let viewed: Vec<String> = st
        .viewed_by_path
        .iter()
        .filter(|(_, v)| **v)
        .map(|(p, _)| p.clone())
        .collect();
    st.status = format!("Pushing to {}…", pr.head);
    submit(
        st,
        tx,
        Job::PushEdits {
            wt: st.active_worktree.clone(),
            repo_root: st.repo_root.clone(),
            owner: st.repo_owner.clone(),
            name: st.repo_name.clone(),
            branch: pr.head,
            pr_id: pr.node_id,
            viewed,
        },
    );
}


/// Select the file another tool asked us to show, once we can.
///
/// The request can arrive before the edit list has loaded, so it is held and
/// retried rather than dropped. A file that is not in the list — because it has
/// no uncommitted changes — clears the request instead of retrying forever.
pub fn try_open_pending_file(st: &mut State, tx: &std::sync::mpsc::Sender<Job>) {
    let Some(wanted) = st.pending_open_file.clone() else { return };

    // Nothing to match against yet: ask for the edits and wait.
    if st.edit_files.is_empty() {
        if !st.busy.contains("edits") {
            reload_edits(st, tx);
        }
        return;
    }

    st.focus = Focus::Edits;
    match find_edit_row(st, &wanted) {
        Some(idx) => {
            st.pending_open_file = None;
            st.edit_idx = idx;
            st.edit_diff_scroll = 0;
            enter_local_diff(st);
            st.status = format!("opened {wanted}");
        }
        None => {
            st.pending_open_file = None;
            st.status = format!("{wanted} has no uncommitted changes");
        }
    }
}

/// Select the commit another tool asked for, once the PR holding it is loaded.
///
/// Held rather than dropped while `commits` is empty: the request can arrive
/// before (or instead of) a PR being opened, and dropping it would make the
/// caller's Enter do nothing at all.
pub fn try_open_pending_commit(st: &mut State, tx: &Sender<Job>) {
    let Some(wanted) = st.pending_open_commit.clone() else { return };
    if st.commits.is_empty() {
        return;
    }
    // The caller has an abbreviated hash; we hold full oids.
    match st.commits.iter().position(|c| c.oid.starts_with(&wanted)) {
        Some(i) => {
            st.pending_open_commit = None;
            st.focus = Focus::Commits;
            st.commit_idx = i;
            st.commit_selected = std::iter::once(st.commits[i].oid.clone()).collect();
            apply_commit_selection(st, tx);
        }
        None => {
            st.pending_open_commit = None;
            st.status = format!("commit {wanted} is not in this PR");
        }
    }
}

/// The row in the edit tree showing `path`.
pub fn find_edit_row(st: &State, path: &str) -> Option<usize> {
    st.edit_tree.iter().position(|row| match row {
        TreeRow::File { index, .. } => st.edit_files.get(*index).is_some_and(|e| e.path == path),
        TreeRow::Dir { .. } => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Category, EditEntry, EditKind, FileEntry, PendingComment, Pr};

    fn pr(number: i64) -> Pr {
        Pr {
            number,
            title: "T".into(),
            head: "h".into(),
            author: "a".into(),
            node_id: String::new(),
            category: Category::CheckedOut,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn commit(oid: &str) -> crate::models::Commit {
        crate::models::Commit {
            oid: oid.into(),
            headline: "H".into(),
            body: String::new(),
            author: "a".into(),
            date: String::new(),
        }
    }

    /// The manager holds abbreviated hashes; we hold full oids.
    #[test]
    fn a_requested_commit_is_matched_on_its_short_hash() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut st = State::default();
        st.active_pr = Some(pr(1));
        st.commits = vec![commit("aaaaaaaaaaaa"), commit("bbbbbbbbbbbb")];
        st.pending_open_commit = Some("bbbbbbb".into());

        try_open_pending_commit(&mut st, &tx);
        assert_eq!(st.commit_idx, 1);
        assert_eq!(st.focus, Focus::Commits);
        assert!(st.commit_selected.contains("bbbbbbbbbbbb"));
        assert!(st.commit_selected.len() == 1, "only the one asked for");
        assert!(st.pending_open_commit.is_none(), "the request is spent");
    }

    /// The request can arrive before a PR is open; dropping it then would make
    /// the caller's Enter do nothing at all.
    #[test]
    fn a_commit_request_waits_for_a_pr_to_load() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut st = State::default();
        st.pending_open_commit = Some("abc".into());
        try_open_pending_commit(&mut st, &tx);
        assert_eq!(st.pending_open_commit.as_deref(), Some("abc"), "still waiting");
    }

    /// A commit from another branch must say so rather than sitting there
    /// looking like it is still loading.
    #[test]
    fn a_commit_that_is_not_in_the_pr_is_reported() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut st = State::default();
        st.active_pr = Some(pr(1));
        st.commits = vec![commit("aaaaaaaaaaaa")];
        st.pending_open_commit = Some("ffffff".into());

        try_open_pending_commit(&mut st, &tx);
        assert!(st.pending_open_commit.is_none(), "not retried forever");
        assert!(st.status.contains("not in this PR"), "{}", st.status);
    }

    #[test]
    fn merge_adds_edit_only_files_without_dups() {
        let mut st = State::default();
        st.pr_files = vec![
            FileEntry { path: "a.rs".into(), viewed: true },
            FileEntry { path: "b.rs".into(), viewed: false },
        ];
        st.edit_files = vec![
            EditEntry { path: "b.rs".into(), kind: EditKind::Modified }, // already a PR file
            EditEntry { path: "new.rs".into(), kind: EditKind::Added },  // edit-only
        ];
        merge_edit_files_into_tree(&mut st);
        let paths: Vec<&str> = st.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"a.rs") && paths.contains(&"b.rs") && paths.contains(&"new.rs"));
        assert_eq!(st.files.iter().filter(|f| f.path == "b.rs").count(), 1, "no duplicate");
        // Existing PR files keep their viewed flag.
        assert!(st.files.iter().find(|f| f.path == "a.rs").unwrap().viewed);
    }

    #[test]
    fn review_prompt_lists_each_comment() {
        let mut st = State::default();
        st.active_pr = Some(pr(7));
        st.pending = vec![
            PendingComment {
                path: "x.rs".into(), body: "fix this".into(), line: 10,
                side: "RIGHT".into(), comment_id: String::new(),
                start_line: None, start_side: String::new(),
            },
            PendingComment {
                path: "y.rs".into(), body: "and that".into(), line: 5,
                side: "RIGHT".into(), comment_id: String::new(),
                start_line: Some(3), start_side: "RIGHT".into(),
            },
        ];
        let p = build_review_prompt(&st, "overall note");
        assert!(p.contains("#7"));
        assert!(p.contains("overall note"));
        assert!(p.contains("x.rs:10") && p.contains("fix this"));
        assert!(p.contains("y.rs:3-5") && p.contains("and that"));
    }

    /// The socket hands us a path; finding its row is what turns that into a
    /// selection, and a directory row must never be taken for a file.
    #[test]
    fn a_pending_file_is_found_by_its_path() {
        let mut st = State::default();
        st.edit_files = vec![
            EditEntry { path: "src/a.rs".into(), kind: EditKind::Modified },
            EditEntry { path: "src/b.rs".into(), kind: EditKind::Added },
        ];
        st.edit_tree = vec![
            TreeRow::Dir { depth: 0, name: "src".into(), path: "src".into(), collapsed: false },
            TreeRow::File { depth: 1, name: "a.rs".into(), index: 0 },
            TreeRow::File { depth: 1, name: "b.rs".into(), index: 1 },
        ];
        assert_eq!(find_edit_row(&st, "src/a.rs"), Some(1));
        assert_eq!(find_edit_row(&st, "src/b.rs"), Some(2));
        assert_eq!(find_edit_row(&st, "src"), None, "a directory is not a file");
        assert_eq!(find_edit_row(&st, "src/missing.rs"), None);
    }

    /// A file with no uncommitted changes clears the request rather than
    /// retrying every frame forever.
    #[test]
    fn a_file_that_is_not_modified_clears_the_request() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut st = State::default();
        st.edit_files = vec![EditEntry { path: "src/a.rs".into(), kind: EditKind::Modified }];
        st.edit_tree = vec![TreeRow::File { depth: 0, name: "a.rs".into(), index: 0 }];
        st.pending_open_file = Some("src/untouched.rs".into());

        try_open_pending_file(&mut st, &tx);
        assert!(st.pending_open_file.is_none(), "it gave up rather than looping");
        assert!(st.status.contains("no uncommitted changes"), "{}", st.status);
    }

    /// Arriving before the list has loaded must hold the request, not drop it.
    #[test]
    fn a_request_that_arrives_early_is_held() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut st = State::default();
        st.pending_open_file = Some("src/a.rs".into());
        try_open_pending_file(&mut st, &tx);
        assert_eq!(st.pending_open_file.as_deref(), Some("src/a.rs"), "still waiting");
    }
}
