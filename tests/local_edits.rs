//! End-to-end checks of the pending-edits pane against a real git repo:
//! staging whole files, committing what the index holds, and reverting one
//! block of a local diff.

use std::path::Path;
use std::process::Command;

use ghreview::api;
use ghreview::models::CommitKind;
use ghreview::diff::{build_hunk_patch, compute_hunks, parse_diff};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repo whose `f.txt` is committed as `a b c d e` and locally edited on two
/// separate lines (two change blocks in one `@@` section).
fn repo(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("ghr-stage-test-{name}"));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "a\nb\nc\nd\ne\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("f.txt"), "a\nB\nc\nD\ne\n").unwrap();
    dir.display().to_string()
}

#[test]
fn an_amend_folds_the_work_into_head_instead_of_adding_a_commit() {
    let wt = repo("amend");
    let before = git(Path::new(&wt), &["rev-list", "--count", "HEAD"]);
    api::commit_edit_files(&wt, "first", &["f.txt".to_string()], CommitKind::NoVerify, &mut |_| {})
        .unwrap();
    let after_commit = git(Path::new(&wt), &["rev-list", "--count", "HEAD"]);
    assert_ne!(before.trim(), after_commit.trim(), "a commit was added");

    std::fs::write(Path::new(&wt).join("f.txt"), "amended\n").unwrap();
    let done = api::commit_edit_files(
        &wt,
        "first, said better",
        &["f.txt".to_string()],
        CommitKind::Amend,
        &mut |_| {},
    )
    .unwrap();
    assert!(done.ok);
    let after_amend = git(Path::new(&wt), &["rev-list", "--count", "HEAD"]);
    assert_eq!(after_commit.trim(), after_amend.trim(), "no second commit");
    assert_eq!(api::head_message(&wt), "first, said better");

    // With nothing left to stage an amend is a reword, which must still happen.
    let done =
        api::commit_edit_files(&wt, "reworded", &[], CommitKind::Amend, &mut |_| {}).unwrap();
    assert!(done.ok);
    assert_eq!(done.files, 0);
    assert_eq!(api::head_message(&wt), "reworded");
    assert_eq!(
        git(Path::new(&wt), &["rev-list", "--count", "HEAD"]).trim(),
        after_amend.trim(),
        "still no second commit"
    );
    std::fs::remove_dir_all(&wt).ok();
}

/// Hook output is worth reading while it happens; a window that fills only at
/// the end is the thing this replaces.
#[test]
fn command_output_arrives_line_by_line() {
    let wt = repo("stream");
    let mut lines = Vec::new();
    let ok = api::sh_stream(
        &["git", "-C", &wt, "log", "--oneline", "--format=%s"],
        &mut |l| lines.push(l),
    )
    .unwrap();
    assert!(ok);
    assert!(lines.iter().any(|l| l == "init"), "{lines:?}");

    // A failure is reported as such rather than as an error, since the output
    // explaining it has already been handed over.
    let mut err = Vec::new();
    let ok = api::sh_stream(&["git", "-C", &wt, "cat-file", "-e", "deadbeef"], &mut |l| err.push(l))
        .unwrap();
    assert!(!ok, "the command failed");
    std::fs::remove_dir_all(&wt).ok();
}

#[test]
fn commit_takes_the_index_when_something_is_staged() {
    let wt = repo("commit-staged");
    // Stage the file as it stands, then change it again: the index holds one
    // version and the working tree another.
    api::stage_paths(&wt, &["f.txt".to_string()], false).unwrap();
    std::fs::write(Path::new(&wt).join("f.txt"), "a\nB\nc\nD\nE\n").unwrap();
    let edits = api::load_edits(&wt);
    assert!(edits.staged.contains("f.txt") && edits.unstaged.contains("f.txt"));

    let done = api::commit_edit_files(
        &wt,
        "partial",
        &["f.txt".to_string()],
        CommitKind::NoVerify,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(done.files, 1);
    assert!(done.ok);
    // Only what was staged went in; the rest is still a local edit.
    let (files, _) = parse_diff(&git(Path::new(&wt), &["show", "HEAD", "-p", "-U0"]));
    assert!(files["f.txt"].iter().any(|l| l == "+D"));
    assert!(!files["f.txt"].iter().any(|l| l == "+E"));
    assert!(api::load_edits(&wt).unstaged.contains("f.txt"));
    std::fs::remove_dir_all(&wt).ok();
}

/// `d` on a block of a file with nothing staged: that block goes back to what
/// HEAD has, and the other one stays.
#[test]
fn reverting_a_block_leaves_the_other_alone() {
    let wt = repo("revert-unstaged");
    let edits = api::load_edits(&wt);
    let lines = &edits.combined.0["f.txt"];
    let blocks = compute_hunks(lines);
    // Built against the post-image (the working tree), which is what it is
    // being undone from; a forwards patch would not apply while b → B is there.
    let patch = build_hunk_patch(lines, blocks[1], true).unwrap();

    api::apply_patch(&wt, &patch, true, api::PatchTarget::Worktree).unwrap();

    let text = std::fs::read_to_string(Path::new(&wt).join("f.txt")).unwrap();
    assert_eq!(text, "a\nB\nc\nd\ne\n", "d → D undone, b → B kept");
    std::fs::remove_dir_all(&wt).ok();
}

/// `d` on a block of a file with something staged has to take it out of the
/// index and the working tree both: leaving the index alone would put the
/// change back on the next commit.
#[test]
fn reverting_a_block_of_a_staged_file_takes_it_off_disk_as_well() {
    let wt = repo("revert-staged");
    api::stage_paths(&wt, &["f.txt".to_string()], false).unwrap();
    let edits = api::load_edits(&wt);
    let lines = &edits.combined.0["f.txt"];
    let blocks = compute_hunks(lines);
    let patch = build_hunk_patch(lines, blocks[1], true).unwrap();

    api::apply_patch(&wt, &patch, true, api::PatchTarget::Both).unwrap();

    let text = std::fs::read_to_string(Path::new(&wt).join("f.txt")).unwrap();
    assert_eq!(text, "a\nB\nc\nd\ne\n", "gone from the working tree too");
    // The other block is still staged: reverting one hunk is not unstaging.
    let staged = git(Path::new(&wt), &["diff", "--cached", "-U0", "--", "f.txt"]);
    assert!(staged.contains("+B"), "{staged}");
    assert!(!staged.contains("+D"), "{staged}");
    std::fs::remove_dir_all(&wt).ok();
}

#[test]
fn discard_reverts_the_index_too() {
    let wt = repo("discard");
    api::stage_paths(&wt, &["f.txt".to_string()], false).unwrap();

    api::discard_edit(&wt, "f.txt", false).unwrap();
    let after = api::load_edits(&wt);
    assert!(after.files.is_empty(), "{:?}", after.files);
    assert_eq!(std::fs::read_to_string(format!("{wt}/f.txt")).unwrap(), "a\nb\nc\nd\ne\n");
    std::fs::remove_dir_all(&wt).ok();
}
