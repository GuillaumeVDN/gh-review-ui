//! External-editor integration (open a file at a line in a running nvim).

use std::process::{Command, Stdio};

use crate::gh::sh;
use crate::models::State;
use crate::navigation::{cur_file_path, current_hunk_editor_line};

/// Fire-and-forget: open `abs_path` in the running nvim server at `line`.
/// Wired for an Omarchy/Hyprland + Neovim setup (nvim on `/tmp/nvim.sock`).
///
/// Uses a single `:edit +{line} {file}` command so the buffer switch and the
/// line jump happen atomically — two separate `--remote`/`--remote-send` calls
/// race, sometimes jumping in the previous buffer (wrong file).
pub fn open_in_editor(abs_path: &str, line: i64) {
    let ex_path = abs_path.replace(' ', "\\ "); // escape spaces for the ex command
    let script = format!(
        "nvim --server /tmp/nvim.sock --remote-send \"<C-\\><C-N>:edit +{line} {ex_path}<CR>\" \
         && (hyprctl dispatch focuswindow class:org.omarchy.nvim | grep -q ok \
             || hyprctl dispatch focuswindow title:^n$)"
    );
    let _ = Command::new("/usr/bin/bash")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn git_head(dir: &str) -> Option<String> {
    sh(&["git", "-C", dir, "rev-parse", "HEAD"]).ok().map(|s| s.trim().to_string())
}

/// Local checkouts that might already hold the PR head: the repo the app was
/// launched from, and the conventional `~/Projects/<repo-name>` clone.
fn candidate_checkouts(st: &State) -> Vec<String> {
    let mut v = Vec::new();
    let repo = st.repo_root.trim_end_matches('/').to_string();
    if !repo.is_empty() {
        v.push(repo);
    }
    if !st.repo_name.is_empty() {
        if let Ok(home) = std::env::var("HOME") {
            let p = format!("{home}/Projects/{}", st.repo_name);
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    v
}

/// Where to open the file for editing.
///
/// Prefer a local checkout already sitting on the PR's head commit (ignoring
/// uncommitted changes) — the repo the app was launched from, or the matching
/// `~/Projects/<repo-name>` clone — since that's the same code the user works
/// on. Otherwise fall back to the PR's review worktree.
fn editor_root(st: &State) -> String {
    let wt = st.active_worktree.trim_end_matches('/').to_string();
    // PR head = the commit the worktree is checked out at.
    let pr_head = st
        .commits
        .first()
        .map(|c| c.oid.clone())
        .or_else(|| if wt.is_empty() { None } else { git_head(&wt) });
    if let Some(head) = pr_head {
        for cand in candidate_checkouts(st) {
            if git_head(&cand).as_deref() == Some(head.as_str()) {
                return cand;
            }
        }
    }
    if !wt.is_empty() {
        wt
    } else if !st.repo_root.is_empty() {
        st.repo_root.trim_end_matches('/').to_string()
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
