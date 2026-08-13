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

/// Open a PR in the default web browser (fire-and-forget).
pub fn open_pr_in_browser(owner: &str, name: &str, number: i64) {
    let repo = format!("{owner}/{name}");
    let _ = Command::new("gh")
        .args(["-R", &repo, "pr", "view", &number.to_string(), "--web"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn spawn_bash(script: &str, cwd: &str) {
    let mut c = Command::new("/usr/bin/bash");
    c.arg("-c").arg(script);
    if !cwd.is_empty() {
        c.current_dir(cwd);
    }
    let _ = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// A stable per-worktree id (…/worktrees/owner__repo/pr-N → owner__repo__pr-N).
fn worktree_id(worktree: &str) -> String {
    let comps: Vec<&str> = worktree.trim_end_matches('/').rsplit('/').take(2).collect();
    let raw = format!("{}__{}", comps.get(1).unwrap_or(&""), comps.first().unwrap_or(&""));
    raw.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

/// Whether a nvim server is already listening on `sock`.
fn nvim_server_alive(sock: &str) -> bool {
    Command::new("nvim")
        .args(["--server", sock, "--remote-expr", "1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether the focused Hyprland window is already part of a group.
fn in_group() -> bool {
    Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v["grouped"].as_array().map(|a| !a.is_empty()))
        .unwrap_or(false)
}

/// Whether a per-worktree Neovim socket file still exists (cheap liveness proxy;
/// Neovim removes it on exit).
pub fn socket_exists(sock: &str) -> bool {
    std::path::Path::new(sock).exists()
}

/// Ask a per-worktree Neovim to quit (closes its Ghostty window).
pub fn close_worktree_editor(sock: &str) {
    let script = format!("nvim --server {sock} --remote-send \"<C-\\><C-N>:qa!<CR>\"");
    spawn_bash(&script, "");
}

/// Dissolve the current Hyprland group if the active window is grouped.
pub fn ungroup_active() {
    if in_group() {
        let _ = Command::new("hyprctl").args(["dispatch", "togglegroup"]).output();
    }
}

/// Open `abs_path` at `line` in a per-worktree Neovim, launched in its own
/// Ghostty window (grouped as a tab beside the TUI) the first time. Subsequent
/// opens reuse that Neovim over its dedicated socket. Rooted at the worktree so
/// project-wide search/replace works.
pub fn open_in_worktree_editor(st: &mut State, worktree: &str, abs_path: &str, line: i64) {
    let id = worktree_id(worktree);
    let sock = format!("/tmp/nvim-ghr-{id}.sock");
    let title = format!("ghr:{id}");
    st.worktree_editors.entry(sock.clone()).or_insert(false);
    if nvim_server_alive(&sock) {
        // Already open: jump to the file/line and focus its window.
        let ex_path = abs_path.replace(' ', "\\ ");
        let script = format!(
            "nvim --server {sock} --remote-send \"<C-\\><C-N>:edit +{line} {ex_path}<CR>\" ; \
             hyprctl dispatch focuswindow title:{title}"
        );
        spawn_bash(&script, worktree);
        return;
    }
    // First open for this worktree: make sure the TUI window forms a group so the
    // new terminal opens as a tab beside it, then launch Ghostty + Neovim.
    if !in_group() {
        let _ = Command::new("hyprctl").args(["dispatch", "togglegroup"]).output();
        st.entered_group = true;
    }
    let _ = Command::new("ghostty")
        .arg(format!("--title={title}"))
        .arg("-e")
        .arg("nvim")
        .arg("--listen")
        .arg(&sock)
        .arg("-c")
        .arg("set notitle")
        .arg(format!("+{line}"))
        // Once startup has settled (file loaded, its own cursor-restore autocmds
        // done), open the file tree, return focus to the file, and re-apply the
        // target line — otherwise a config's "restore last position" autocmd wins.
        .arg("-c")
        .arg(format!(
            "lua vim.defer_fn(function() \
               pcall(vim.cmd,'NvimTreeOpen'); \
               pcall(vim.cmd,'wincmd p'); \
               pcall(vim.api.nvim_win_set_cursor,0,{{{line},0}}); \
               pcall(vim.cmd,'normal! zz') \
             end, 150)"
        ))
        .arg(abs_path)
        .current_dir(worktree)
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
    let wt = st.active_worktree.trim_end_matches('/').to_string();
    let abs = format!("{}/{}", wt, entry.path);
    if st.local_mode {
        open_in_editor(&abs, 1); // main repo → the user's usual nvim
    } else {
        open_in_worktree_editor(st, &wt, &abs, 1);
    }
    st.status = format!("Opening {} in editor…", entry.path);
}

/// Open the selected file — at the top, or at the current hunk's line.
///
/// When the file resolves to a local checkout already on the PR head (the launch
/// repo / `~/Projects/<repo>`), reuse the shared `/tmp/nvim.sock` editor. When it
/// resolves to the review worktree, use a dedicated per-worktree Neovim window.
pub fn open_current_in_editor(st: &mut State, top: bool) {
    let Some(path) = cur_file_path(st) else {
        st.status = "No file selected.".into();
        return;
    };
    let line = if top { 1 } else { current_hunk_editor_line(st, &path) };
    let root = editor_root(st).trim_end_matches('/').to_string();
    let abs = format!("{root}/{path}");
    let wt = st.active_worktree.trim_end_matches('/').to_string();
    if !st.local_mode && !wt.is_empty() && root == wt {
        open_in_worktree_editor(st, &wt, &abs, line);
    } else {
        open_in_editor(&abs, line);
    }
    st.status = format!("Opening {path}:{line} in editor…");
}
