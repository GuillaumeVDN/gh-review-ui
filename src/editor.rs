//! External-editor integration (open a file at a line in a running nvim).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::gh::sh;
use crate::models::{State, TreeRow};
use crate::navigation::{current_hunk_editor_line, diff_path};

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

const REVIEW_TITLE: &str = "ghr-review-claude";

fn has_cmd(cmd: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {cmd}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn window_exists(title: &str) -> bool {
    Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v.as_array().map(|a| a.iter().any(|w| w["title"].as_str() == Some(title))))
        .unwrap_or(false)
}

/// Hand a whole review (all pending comments) to a local Claude.
///
/// If a review-Claude window is already open, focus it and (via `wtype`) type a
/// short instruction pointing at a file that holds the full prompt, then Enter —
/// so a multi-line prompt doesn't submit itself line by line. Otherwise launch a
/// fresh window seeded with the prompt (worktree PR: grouped beside the TUI;
/// local PR: workspace 4).
pub fn send_review_to_claude(st: &mut State, prompt: &str) -> Result<(), String> {
    let cwd = if st.active_worktree.is_empty() { st.repo_root.clone() } else { st.active_worktree.clone() };
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    // Reuse a running review window if we can drive the keyboard.
    if window_exists(REVIEW_TITLE) && has_cmd("wtype") {
        let file = dir.join("review-latest.txt");
        std::fs::write(&file, prompt).map_err(|e| e.to_string())?;
        let fp = file.display().to_string();
        let instruction =
            format!("Read {fp} and address all the review comments described in it.");
        let script = format!(
            "hyprctl dispatch focuswindow title:^{REVIEW_TITLE}$; sleep 0.2; \
             wtype {q}; wtype -k Return",
            q = shell_single_quote(&instruction),
        );
        spawn_bash(&script, "");
        return Ok(());
    }

    // Otherwise open a fresh window seeded with the whole prompt.
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let file = dir.join(format!("review-{ts}.txt"));
    std::fs::write(&file, prompt).map_err(|e| e.to_string())?;
    let fp = file.display().to_string();
    let script =
        format!("p=\"$(cat '{fp}')\"; rm -f '{fp}'; exec claude --dangerously-skip-permissions \"$p\"");
    if st.local_mode {
        // Place it on workspace 4 silently via a window rule matched on the title.
        let _ = Command::new("hyprctl")
            .args(["keyword", "windowrulev2", &format!("workspace 4 silent, title:^{REVIEW_TITLE}$")])
            .output();
    } else if !in_group() {
        let _ = Command::new("hyprctl").args(["dispatch", "togglegroup"]).output();
        st.entered_group = true;
    }
    Command::new("ghostty")
        .arg(format!("--title={REVIEW_TITLE}"))
        .arg("-e")
        .arg("bash")
        .arg("-lc")
        .arg(script)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Wrap `s` in single quotes for a bash command (handling embedded quotes).
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn spawn_bash(script: &str, cwd: &str) {
    let mut c = Command::new("/usr/bin/bash");
    c.arg("-c").arg(script);
    if !cwd.is_empty() {
        c.current_dir(cwd);
    }
    let _ = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// A stable per-checkout id (…/worktrees/owner__repo/pr-N → owner__repo__pr-N,
/// …/Projects/gh-review-ui → Projects__gh-review-ui).
fn worktree_id(root: &str) -> String {
    let comps: Vec<&str> = root.trim_end_matches('/').rsplit('/').take(2).collect();
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

/// Open `abs_path` at `line` in a per-checkout Neovim, launched in its own
/// Ghostty window (grouped as a tab beside the TUI) the first time. Subsequent
/// opens reuse that Neovim over its dedicated socket. Rooted at `root` (a review
/// worktree or the main repo) so project-wide search/replace works.
pub fn open_in_dedicated_editor(st: &mut State, root: &str, abs_path: &str, line: i64) {
    let id = worktree_id(root);
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
        spawn_bash(&script, root);
        return;
    }
    // First open for this checkout: make sure the TUI window forms a group so the
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
        .current_dir(root)
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
    open_in_dedicated_editor(st, &wt, &abs, 1);
    st.status = format!("Opening {} in editor…", entry.path);
}

/// Open the selected file — at the top, or at the current hunk's line.
///
/// Always in a dedicated Neovim window grouped beside the TUI, whether the file
/// resolves to the review worktree or to a local checkout already on the PR head
/// (the launch repo / `~/Projects/<repo>`).
pub fn open_current_in_editor(st: &mut State, top: bool) {
    let Some(path) = diff_path(st) else {
        st.status = "No file selected.".into();
        return;
    };
    let line = if top { 1 } else { current_hunk_editor_line(st, &path) };
    let root = editor_root(st).trim_end_matches('/').to_string();
    let abs = format!("{root}/{path}");
    open_in_dedicated_editor(st, &root, &abs, line);
    st.status = format!("Opening {path}:{line} in editor…");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_single_quote("abc"), "'abc'");
        assert_eq!(shell_single_quote("a b"), "'a b'");
        // An embedded single quote is closed, escaped, reopened.
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn worktree_id_from_path() {
        assert_eq!(worktree_id("/x/worktrees/owner__repo/pr-35"), "owner__repo__pr-35");
        assert_eq!(worktree_id("/x/worktrees/owner__repo/pr-35/"), "owner__repo__pr-35");
    }
}
