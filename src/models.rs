//! Domain data types and the central [`State`].

use std::collections::{HashMap, HashSet};

use crate::textbuffer::TextArea;

/// (old_line_no, new_line_no) for a diff row; `None` where the row doesn't
/// exist on that side.
pub type LineInfo = (Option<i64>, Option<i64>);
/// A hunk/block range into a file's diff-line list: `[start, end)`.
pub type Range = (usize, usize);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Mine,
    Review,
}

#[derive(Clone, Debug)]
pub struct Pr {
    pub number: i64,
    pub title: String,
    pub head: String,
    pub author: String,
    pub node_id: String,
    pub category: Category,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub oid: String,
    pub headline: String,
    pub body: String,
    pub author: String,
    pub date: String,
}

impl Commit {
    pub fn short(&self) -> &str {
        let n = self.oid.len().min(7);
        &self.oid[..n]
    }
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub viewed: bool,
}

#[derive(Clone, Debug)]
pub struct PendingComment {
    pub path: String,
    pub body: String,
    pub line: i64,
    pub side: String, // "RIGHT" | "LEFT"
    pub comment_id: String,
    pub start_line: Option<i64>,
    pub start_side: String,
}

/// A tree row for the Files pane.
#[derive(Clone, Debug)]
pub enum TreeRow {
    Dir { depth: usize, name: String, path: String, collapsed: bool },
    File { depth: usize, name: String, index: usize },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Prs,
    Commits,
    Files,
    Pending,
    Diff,
}

/// Tab-cycle order (matches top-to-bottom pane layout, diff last).
pub const FOCUS_ORDER: [Focus; 5] =
    [Focus::Prs, Focus::Commits, Focus::Files, Focus::Pending, Focus::Diff];

impl Focus {
    pub fn next(self) -> Focus {
        let i = FOCUS_ORDER.iter().position(|&f| f == self).unwrap_or(0);
        FOCUS_ORDER[(i + 1) % FOCUS_ORDER.len()]
    }
    pub fn prev(self) -> Focus {
        let i = FOCUS_ORDER.iter().position(|&f| f == self).unwrap_or(0);
        FOCUS_ORDER[(i + FOCUS_ORDER.len() - 1) % FOCUS_ORDER.len()]
    }
    pub fn from_digit(d: char) -> Option<Focus> {
        match d {
            '0' => Some(Focus::Diff),
            '1' => Some(Focus::Prs),
            '2' => Some(Focus::Commits),
            '3' => Some(Focus::Files),
            '4' => Some(Focus::Pending),
            _ => None,
        }
    }
}

/// The finish-review dialog's chosen event, in display order.
pub const REVIEW_EVENTS: [(&str, &str); 3] = [
    ("COMMENT", "Comment"),
    ("REQUEST_CHANGES", "Request changes"),
    ("APPROVE", "Approve"),
];

/// A modal overlay drawn on top of the panes and owning keyboard input.
#[derive(Clone, Debug)]
pub enum Overlay {
    None,
    /// New comment on the picked line/range.
    Comment {
        ta: TextArea,
        path: String,
        line: i64,
        side: String,
        start_line: Option<i64>,
        start_side: String,
    },
    /// Edit an existing pending comment.
    Edit { ta: TextArea, comment_id: String, path: String, line: i64 },
    /// Finish-review: description editor (top) + event choice (bottom).
    Review { ta: TextArea, editing: bool, choice: usize },
}

#[derive(Default)]
pub struct State {
    pub repo_owner: String,
    pub repo_name: String,
    pub repo_root: String,
    pub viewer: String,

    pub prs: Vec<Pr>,
    pub pr_idx: usize,
    pub pr_offset: usize,
    pub active_pr: Option<Pr>,
    pub active_worktree: String,

    pub commits: Vec<Commit>,
    pub commit_selected: HashSet<String>,
    pub commit_idx: usize,
    pub commit_offset: usize,

    pub files: Vec<FileEntry>,
    pub viewed_by_path: HashMap<String, bool>,
    pub collapsed_dirs: HashSet<String>,
    pub tree: Vec<TreeRow>,
    pub file_idx: usize,
    pub file_offset: usize,

    pub diff_by_file: HashMap<String, Vec<String>>,
    pub info_by_file: HashMap<String, Vec<LineInfo>>,
    pub hunks_by_file: HashMap<String, Vec<Range>>,
    pub diff_scroll: usize,
    pub diff_hunk_idx: usize,
    /// Set when a keyboard action should scroll the selected hunk/comment into
    /// view on the next render; free scrolling (mouse/PgUp/Dn) leaves it unset.
    pub diff_reveal_pending: bool,

    // In-hunk comment picker.
    pub comment_mode: bool,
    pub comment_line: usize,
    pub comment_start: Option<usize>,
    /// In-progress comment text, keyed by file path, so a draft survives closing
    /// the editor and switching files. Restored when commenting that file again;
    /// dropped when the comment is submitted (or emptied).
    pub comment_drafts: HashMap<String, String>,
    /// (file path, end diff-line index) of the last submitted comment, so the
    /// next `c` on that file starts on the following line.
    pub last_comment: Option<(String, usize)>,

    pub pr_details: HashMap<i64, Option<serde_json::Value>>,
    pub details_scroll: usize,

    pub pending: Vec<PendingComment>,
    pub pending_idx: usize,
    pub pending_offset: usize,

    pub focus: Focus,
    pub status: String,
    pub busy: HashSet<String>,
    pub overlay: Overlay,
    pub should_quit: bool,
}

impl Default for Focus {
    fn default() -> Self {
        Focus::Prs
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Overlay::None
    }
}
