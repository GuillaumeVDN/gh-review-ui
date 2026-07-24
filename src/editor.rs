//! External-editor integration (open a file at a line in a running nvim).

use std::process::{Command, Stdio};

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

/// Open the selected file — at the top, or at the current hunk's line.
pub fn open_current_in_editor(st: &mut State, top: bool) {
    let Some(path) = cur_file_path(st) else {
        st.status = "No file selected.".into();
        return;
    };
    let line = if top { 1 } else { current_hunk_editor_line(st, &path) };
    // Prefer the PR's review worktree so edits land on the reviewed branch.
    let root = if !st.active_worktree.is_empty() {
        st.active_worktree.clone()
    } else if !st.repo_root.is_empty() {
        st.repo_root.clone()
    } else {
        ".".to_string()
    };
    let abs = format!("{}/{}", root.trim_end_matches('/'), path);
    open_in_editor(&abs, line);
    st.status = format!("Opening {path}:{line} in editor…");
}
