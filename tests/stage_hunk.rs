//! End-to-end check of hunk staging against a real git repo: the synthesised
//! one-block patches must apply to (and reverse out of) the index.

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
fn stages_then_unstages_a_single_block() {
    let wt = repo("single-block");
    let edits = api::load_edits(&wt);
    let lines = &edits.unstaged.0["f.txt"];
    let blocks = compute_hunks(lines);
    assert_eq!(blocks.len(), 2, "two separate change blocks");

    // Stage only the second block (d → D).
    let patch = build_hunk_patch(lines, blocks[1], false).unwrap();
    api::apply_index_patch(&wt, &patch, false).unwrap();

    let after = api::load_edits(&wt);
    // The index now holds a/b/c/D/e: staged carries D, unstaged still carries B.
    let staged: Vec<&String> = after.staged.0["f.txt"].iter().collect();
    assert!(staged.iter().any(|l| l.as_str() == "+D"), "{staged:?}");
    assert!(!staged.iter().any(|l| l.as_str() == "+B"), "{staged:?}");
    let unstaged: Vec<&String> = after.unstaged.0["f.txt"].iter().collect();
    assert!(unstaged.iter().any(|l| l.as_str() == "+B"), "{unstaged:?}");
    assert!(!unstaged.iter().any(|l| l.as_str() == "+D"), "{unstaged:?}");
    // The worktree is untouched by staging.
    assert_eq!(std::fs::read_to_string(format!("{wt}/f.txt")).unwrap(), "a\nB\nc\nD\ne\n");

    // Unstage it again, from the staged diff this time.
    let staged_lines = &after.staged.0["f.txt"];
    let sblocks = compute_hunks(staged_lines);
    let rpatch = build_hunk_patch(staged_lines, sblocks[0], true).unwrap();
    api::apply_index_patch(&wt, &rpatch, true).unwrap();

    let back = api::load_edits(&wt);
    assert!(!back.staged.0.contains_key("f.txt"), "index back to HEAD");
    assert_eq!(compute_hunks(&back.unstaged.0["f.txt"]).len(), 2);
    std::fs::remove_dir_all(&wt).ok();
}

#[test]
fn stages_part_of_an_untracked_file() {
    let wt = repo("untracked");
    std::fs::write(format!("{wt}/new.txt"), "one\ntwo\n").unwrap();
    let edits = api::load_edits(&wt);
    let lines = &edits.unstaged.0["new.txt"];
    let blocks = compute_hunks(lines);
    // Stage just the first added line.
    let patch = build_hunk_patch(lines, (blocks[0].0, blocks[0].0 + 1), false).unwrap();
    api::apply_index_patch(&wt, &patch, false).unwrap();

    let after = api::load_edits(&wt);
    assert_eq!(git(Path::new(&wt), &["show", ":new.txt"]), "one\n");
    let unstaged: Vec<&String> = after.unstaged.0["new.txt"].iter().collect();
    assert!(unstaged.iter().any(|l| l.as_str() == "+two"), "{unstaged:?}");
    std::fs::remove_dir_all(&wt).ok();
}

/// The hooks run for a plain commit and are skipped for `w`; a temporary repo
/// has none either way, so this pins the outcome shape rather than the hooks.
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
    let edits = api::load_edits(&wt);
    let lines = &edits.unstaged.0["f.txt"];
    let blocks = compute_hunks(lines);
    let patch = build_hunk_patch(lines, blocks[1], false).unwrap();
    api::apply_index_patch(&wt, &patch, false).unwrap();

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
    // Only the staged half went in; the rest is still a local edit.
    let (files, _) = parse_diff(&git(Path::new(&wt), &["show", "HEAD", "-p", "-U0"]));
    assert!(files["f.txt"].iter().any(|l| l == "+D"));
    assert!(!files["f.txt"].iter().any(|l| l == "+B"));
    assert!(api::load_edits(&wt).unstaged.0.contains_key("f.txt"));
    std::fs::remove_dir_all(&wt).ok();
}

#[test]
fn discard_reverts_the_index_too() {
    let wt = repo("discard");
    let edits = api::load_edits(&wt);
    let lines = &edits.unstaged.0["f.txt"];
    let blocks = compute_hunks(lines);
    let patch = build_hunk_patch(lines, blocks[0], false).unwrap();
    api::apply_index_patch(&wt, &patch, false).unwrap();

    api::discard_edit(&wt, "f.txt", false).unwrap();
    let after = api::load_edits(&wt);
    assert!(after.files.is_empty(), "{:?}", after.files);
    assert_eq!(std::fs::read_to_string(format!("{wt}/f.txt")).unwrap(), "a\nb\nc\nd\ne\n");
    std::fs::remove_dir_all(&wt).ok();
}
