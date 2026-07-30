//! State transitions and job orchestration (glue between UI and worker).

use std::sync::mpsc::Sender;

use crate::api;
use crate::diff::compute_hunks;
use crate::models::{
    Category, FileEntry, Overlay, PendingComment, Pr, State, TreeRow, REVIEW_EVENTS,
};
use crate::navigation::{
    cur_file_path, first_change_index, hunk_line_indices, line_target,
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
    st.comment_mode = false;
    st.comment_start = None;
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
                    st.viewed_by_path.clear();
                    st.commits.clear();
                    st.commit_selected.clear();
                    st.commit_idx = 0;
                    st.commit_offset = 0;
                    set_diff(st, Default::default(), Default::default());
                }
                Some(n) => {
                    let matched = st.prs.iter().find(|p| p.number == n).cloned();
                    let mut pr = matched.unwrap_or(Pr {
                        number: n,
                        title: format!("#{n}"),
                        head: String::new(),
                        author: String::new(),
                        node_id: pr_id.clone(),
                        category: Category::Review,
                    });
                    pr.node_id = pr_id;
                    st.active_pr = Some(pr);
                    st.viewed_by_path = files.iter().map(|f| (f.path.clone(), f.viewed)).collect();
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
        }
        Msg::CommitDiff { diff, info } => {
            st.busy.remove("commitdiff");
            let mut paths: Vec<String> = diff.keys().cloned().collect();
            paths.sort();
            st.files = paths
                .iter()
                .map(|p| FileEntry { path: p.clone(), viewed: *st.viewed_by_path.get(p).unwrap_or(&false) })
                .collect();
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
            submit(st, tx, Job::LoadActive { owner, name, login, number: Some(number) });
        }
        Msg::ViewedOk { paths, viewed } => {
            for f in st.files.iter_mut() {
                if paths.contains(&f.path) {
                    f.viewed = viewed;
                }
            }
            for p in &paths {
                st.viewed_by_path.insert(p.clone(), viewed);
            }
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
        Msg::Error { kind, msg } => {
            st.busy.remove(&kind);
            st.status = format!("[{kind}] {msg}");
        }
    }
}

// ---- comment line picker ----

pub fn enter_comment_mode(st: &mut State) {
    if st.active_pr.is_none() {
        st.status = "No active PR.".into();
        return;
    }
    let Some(path) = cur_file_path(st) else { return };
    let idxs = hunk_line_indices(st, &path);
    if idxs.is_empty() {
        st.status = "No commentable line in the current hunk.".into();
        return;
    }
    st.comment_mode = true;
    st.comment_start = None;
    st.comment_line = first_change_index(st, &path).unwrap_or(idxs[0]);
    st.status = "Comment: j/k line · Shift+J/K range · Enter confirm · Esc cancel".into();
}

pub fn move_comment(st: &mut State, direction: i64, extend: bool) {
    let Some(path) = cur_file_path(st) else { return };
    let idxs = hunk_line_indices(st, &path);
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
}

/// From the picker, open the comment editor overlay on the selected line/range.
pub fn begin_comment(st: &mut State) {
    st.comment_mode = false;
    let Some(path) = cur_file_path(st) else { return };
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
    st.overlay = Overlay::Comment { ta: TextArea::new(""), path, line, side, start_line, start_side };
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
    let Some(pr) = st.active_pr.clone() else { return };
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
    let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
    submit(st, tx, Job::EditPending { owner, name, number: pr.number, login, comment_id, body });
    st.status = format!("Updating comment on {path}:{line}…");
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
    let event = REVIEW_EVENTS[choice.min(REVIEW_EVENTS.len() - 1)].0.to_string();
    let body = ta.text();
    let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
    st.status = format!("Submitting review ({event})…");
    submit(st, tx, Job::SubmitReview { owner, name, number: pr.number, login, pr_id: pr.node_id, event, body });
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
                st.status = format!("{} {} files in {path}/…", if new_v { "Marking" } else { "Unmarking" }, paths.len());
                submit(st, tx, Job::MarkViewedBulk { pr_id, paths, viewed: new_v });
            }
        }
    }
}

pub fn fold_viewed(st: &mut State) {
    // Remember the file the cursor sat on so we can jump forward from it (folding
    // may collapse it away, so capture the underlying file index, not the row).
    let anchor = match st.tree.get(st.file_idx) {
        Some(TreeRow::File { index, .. }) => Some(*index),
        Some(TreeRow::Dir { path, .. }) => tree::files_under_dir(st, path).into_iter().next(),
        None => None,
    };
    let folded = tree::fold_viewed_dirs(st);
    let jumped = tree::next_unviewed_index(st, anchor);
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
