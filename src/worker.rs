//! Background worker: runs blocking `gh`/`git` jobs off the UI thread.
//!
//! The UI sends [`Job`]s and receives [`Msg`]s over channels;
//! [`crate::controller`] owns the meaning of both.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};

use serde_json::Value;

use crate::api;
use crate::models::{Commit, EditEntry, FileEntry, LineInfo, PendingComment, Pr};

type Diff = HashMap<String, Vec<String>>;
type Info = HashMap<String, Vec<LineInfo>>;

pub enum Job {
    LoadPrs,
    LoadActive { owner: String, name: String, login: String, number: Option<i64> },
    LoadCommitDiff { first: String, last: String },
    OpenPr { repo_root: String, owner: String, name: String, number: i64 },
    MarkViewed { pr_id: String, path: String, viewed: bool },
    MarkViewedBulk { pr_id: String, paths: Vec<String>, viewed: bool },
    LoadPrDetails(i64),
    AddPending { owner: String, name: String, number: i64, login: String, pr_id: String, comment: PendingComment },
    DiscardPending { owner: String, name: String, number: i64, login: String, comment_id: String },
    EditPending { owner: String, name: String, number: i64, login: String, comment_id: String, body: String },
    SubmitReview { owner: String, name: String, number: i64, login: String, pr_id: String, event: String, body: String },
    LoadEdits { wt: String },
    DiscardEdit { wt: String, path: String, added: bool },
    CommitEdits {
        wt: String,
        repo_root: String,
        owner: String,
        name: String,
        branch: String,
        message: String,
        paths: Vec<String>,
    },
    CheckoutLocal { dir: String, owner: String, name: String, number: i64 },
    Quit,
}

pub enum Msg {
    Prs(Vec<Pr>),
    Active {
        number: Option<i64>,
        pr_id: String,
        files: Vec<FileEntry>,
        diff: Diff,
        info: Info,
        pending: Vec<PendingComment>,
        commits: Vec<Commit>,
    },
    CommitDiff { diff: Diff, info: Info },
    PrOpened { number: i64, path: String },
    ViewedOk { paths: Vec<String>, viewed: bool },
    ViewedBulk { done: Vec<String>, viewed: bool, errs: usize },
    PrDetails { number: i64, data: Value },
    PendingList { pending: Vec<PendingComment>, status: String },
    ReviewSubmitted(String),
    Edits { files: Vec<EditEntry>, diff: Diff, info: Info },
    EditsCommitted { status: String },
    /// Generic "job done" notice: clears `kind` from busy and shows `msg`.
    Done { kind: String, msg: String },
    Error { kind: String, msg: String },
}

/// The "busy" tag a job kind drives, for spinner display.
pub fn job_tag(job: &Job) -> &'static str {
    match job {
        Job::LoadPrs => "prs",
        Job::LoadActive { .. } => "active",
        Job::LoadCommitDiff { .. } => "commitdiff",
        Job::OpenPr { .. } => "worktree",
        Job::MarkViewed { .. } | Job::MarkViewedBulk { .. } => "viewed",
        Job::LoadPrDetails(_) => "details",
        Job::AddPending { .. } | Job::DiscardPending { .. } | Job::EditPending { .. } => "pending",
        Job::SubmitReview { .. } => "review",
        Job::LoadEdits { .. } | Job::DiscardEdit { .. } => "edits",
        Job::CommitEdits { .. } => "editcommit",
        Job::CheckoutLocal { .. } => "checkout",
        Job::Quit => "",
    }
}

fn run(job: &Job) -> anyhow::Result<Msg> {
    Ok(match job {
        Job::LoadPrs => Msg::Prs(api::load_prs()?),
        Job::LoadActive { owner, name, login, number } => match number {
            None => Msg::Active {
                number: None,
                pr_id: String::new(),
                files: vec![],
                diff: HashMap::new(),
                info: HashMap::new(),
                pending: vec![],
                commits: vec![],
            },
            Some(n) => {
                let (pr_id, files) = api::load_files(owner, name, *n)?;
                let commits = api::load_commits(*n).unwrap_or_default();
                let (diff, info) = if let (Some(oldest), Some(newest)) = (commits.last(), commits.first()) {
                    api::load_diff_range(&oldest.oid, &newest.oid)?
                } else {
                    api::load_diff(*n)?
                };
                let pending = if login.is_empty() {
                    vec![]
                } else {
                    api::load_pending_comments(owner, name, *n, login).unwrap_or_default()
                };
                Msg::Active { number: Some(*n), pr_id, files, diff, info, pending, commits }
            }
        },
        Job::LoadCommitDiff { first, last } => {
            let (diff, info) = api::load_diff_range(first, last)?;
            Msg::CommitDiff { diff, info }
        }
        Job::OpenPr { repo_root, owner, name, number } => {
            let path = api::open_pr_worktree(repo_root, owner, name, *number)?;
            Msg::PrOpened { number: *number, path }
        }
        Job::MarkViewed { pr_id, path, viewed } => {
            api::mark_viewed_api(pr_id, path, *viewed)?;
            Msg::ViewedOk { paths: vec![path.clone()], viewed: *viewed }
        }
        Job::MarkViewedBulk { pr_id, paths, viewed } => {
            let (done, errs) = api::mark_viewed_bulk_api(pr_id, paths, *viewed);
            Msg::ViewedBulk { done, viewed: *viewed, errs: errs.len() }
        }
        Job::LoadPrDetails(n) => Msg::PrDetails { number: *n, data: api::load_pr_details(*n)? },
        Job::AddPending { owner, name, number, login, pr_id, comment } => {
            api::add_pending_comment_api(owner, name, *number, login, pr_id, comment)?;
            let pending = api::load_pending_comments(owner, name, *number, login)?;
            Msg::PendingList { pending, status: "Comment added to pending review".into() }
        }
        Job::DiscardPending { owner, name, number, login, comment_id } => {
            if !comment_id.is_empty() {
                api::delete_pending_comment_api(comment_id)?;
            }
            let pending = api::load_pending_comments(owner, name, *number, login)?;
            Msg::PendingList { pending, status: "Discarded pending comment".into() }
        }
        Job::EditPending { owner, name, number, login, comment_id, body } => {
            api::update_pending_comment_api(comment_id, body)?;
            let pending = api::load_pending_comments(owner, name, *number, login)?;
            Msg::PendingList { pending, status: "Comment updated".into() }
        }
        Job::SubmitReview { owner, name, number, login, pr_id, event, body } => {
            api::submit_review_api(owner, name, *number, login, pr_id, event, body)?;
            Msg::ReviewSubmitted(event.clone())
        }
        Job::LoadEdits { wt } => {
            let (files, diff, info) = api::load_edits(wt);
            Msg::Edits { files, diff, info }
        }
        Job::DiscardEdit { wt, path, added } => {
            api::discard_edit(wt, path, *added)?;
            let (files, diff, info) = api::load_edits(wt);
            Msg::Edits { files, diff, info }
        }
        Job::CommitEdits { wt, repo_root, owner, name, branch, message, paths } => {
            let remote = api::base_remote(repo_root, owner, name);
            let pushed = api::commit_edit_files(wt, &remote, branch, message, paths)?;
            Msg::EditsCommitted {
                status: if pushed {
                    format!("Committed {} file(s) and pushed to {branch}", paths.len())
                } else {
                    "No local changes to commit".into()
                },
            }
        }
        Job::CheckoutLocal { dir, owner, name, number } => {
            let msg = api::checkout_pr_local(dir, owner, name, *number)?;
            Msg::Done { kind: "checkout".into(), msg }
        }
        Job::Quit => unreachable!(),
    })
}

pub fn worker_loop(rx: Receiver<Job>, tx: Sender<Msg>) {
    while let Ok(job) = rx.recv() {
        if matches!(job, Job::Quit) {
            return;
        }
        let tag = job_tag(&job).to_string();
        match run(&job) {
            Ok(msg) => {
                let _ = tx.send(msg);
            }
            Err(e) => {
                let _ = tx.send(Msg::Error { kind: tag, msg: format!("{e:#}") });
            }
        }
    }
}
