//! External-editor integration (open a file at a line in a running nvim).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::gh::sh;
use crate::models::{State, TreeRow};
use crate::navigation::{cur_file_path, current_hunk_editor_line};

fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .unwrap_or_else(|_| format!("{}/.cache", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(base).join("gh-review-ui")
}

/// Launch a new Ghostty window running an interactive `claude` seeded with
/// `prompt`, in `cwd` (the PR worktree, so Claude can open the real files).
///
/// The prompt is written to a temp file and read back into claude's argument
/// (`claude "$(cat file)"`) so a large multi-line diff survives intact without
/// depending on env-var propagation; the file is removed once read.
pub fn ask_claude(prompt: &str, cwd: &str) -> Result<(), String> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let file = dir.join(format!("ask-{ts}.txt"));
    std::fs::write(&file, prompt).map_err(|e| e.to_string())?;
    let fp = file.display().to_string();
    // Read the prompt, delete the temp file, then hand off to claude in
    // auto-mode (skip permission prompts so it can read files freely).
    let script =
        format!("p=\"$(cat '{fp}')\"; rm -f '{fp}'; exec claude --dangerously-skip-permissions \"$p\"");
    Command::new("ghostty")
        .arg("-e")
        .arg("bash")
        .arg("-lc")
        .arg(script)
        .current_dir(if cwd.is_empty() { "." } else { cwd })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

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

/// Open the selected pending-edit file in the worktree (where the change lives,
/// so further edits stay detectable and committable on the PR's safe branch).
pub fn open_current_edit_in_editor(st: &mut State) {
    let Some(TreeRow::File { index, .. }) = st.edit_tree.get(st.edit_idx).cloned() else {
        st.status = "No file selected.".into();
        return;
    };
    let Some(entry) = st.edit_files.get(index).cloned() else { return };
    if st.active_worktree.is_empty() {
        st.status = "No worktree.".into();
        return;
    }
    let abs = format!("{}/{}", st.active_worktree.trim_end_matches('/'), entry.path);
    open_in_editor(&abs, 1);
    st.status = format!("Opening {} in editor…", entry.path);
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
