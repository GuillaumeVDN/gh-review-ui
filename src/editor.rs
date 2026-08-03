//! External-editor integration (open a file at a line in a running nvim).

use std::process::{Command, Stdio};

use crate::gh::sh;
use crate::models::State;
use crate::navigation::{cur_file_path, current_hunk_editor_line};

/// Fire-and-forget: open `abs_path` in the running nvim server at `line`.
/// Wired for an Omarchy/Hyprland + Neovim setup (nvim on `/tmp/nvim.sock`).
pub fn open_in_editor(abs_path: &str, line: i64) {
    let script = format!(
        "nvim --server /tmp/nvim.sock --remote {q} \
         && nvim --server /tmp/nvim.sock --remote-send \":{line}<CR>\" \
         && (hyprctl dispatch focuswindow class:org.omarchy.nvim | grep -q ok \
             || hyprctl dispatch focuswindow title:^n$)",
        q = shell_quote(abs_path),
    );
    let _ = Command::new("/usr/bin/bash")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn git_head(dir: &str) -> Option<String> {
    sh(&["git", "-C", dir, "rev-parse", "HEAD"]).ok().map(|s| s.trim().to_string())
}

/// Where to open the file for editing.
///
/// Prefer the PR's review worktree so edits land on the reviewed branch —
/// *unless* the main repo (where the app was launched) is already sitting on
/// the PR's head commit (ignoring uncommitted changes). In that case the user
/// is effectively working the same code, so open the main checkout instead.
fn editor_root(st: &State) -> String {
    let wt = st.active_worktree.trim_end_matches('/').to_string();
    let repo = st.repo_root.trim_end_matches('/').to_string();
    if !wt.is_empty() && !repo.is_empty() {
        // PR head = the worktree's checked-out commit (newest PR commit).
        let pr_head = st.commits.first().map(|c| c.oid.clone()).or_else(|| git_head(&wt));
        if let (Some(a), Some(b)) = (pr_head, git_head(&repo)) {
            if a == b {
                return repo;
            }
        }
    }
    if !wt.is_empty() {
        wt
    } else if !repo.is_empty() {
        repo
    } else {
        ".".to_string()
    }
}

/// Open the selected file — at the top, or at the current hunk's line.
pub fn open_current_in_editor(st: &mut State, top: bool) {
    let Some(path) = cur_file_path(st) else {
        st.status = "No file selected.".into();
        return;
    };
    let line = if top { 1 } else { current_hunk_editor_line(st, &path) };
    let root = editor_root(st);
    let abs = format!("{}/{}", root.trim_end_matches('/'), path);
    open_in_editor(&abs, line);
    st.status = format!("Opening {path}:{line} in editor…");
}
