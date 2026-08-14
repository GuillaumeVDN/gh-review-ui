//! Domain data types and the central [`State`].

use std::collections::{HashMap, HashSet};

use crate::textbuffer::TextArea;

/// (old_line_no, new_line_no) for a diff row; `None` where the row doesn't
/// exist on that side.
pub type LineInfo = (Option<i64>, Option<i64>);
/// A hunk/block range into a file's diff-line list: `[start, end)`.
pub type Range = (usize, usize);
/// Per-file diff lines / line info / change blocks.
pub type DiffMap = HashMap<String, Vec<String>>;
pub type InfoMap = HashMap<String, Vec<LineInfo>>;
pub type HunkMap = HashMap<String, Vec<Range>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    /// The PR currently checked out in the local repo (reviewed in-place).
    CheckedOut,
    Review,
    Mine,
}

#[derive(Clone, Debug)]
pub struct Pr {
    pub number: i64,
    pub title: String,
    pub head: String,
    pub author: String,
    pub node_id: String,
    pub category: Category,
    pub created_at: String,
    pub updated_at: String,
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

/// The kind of local worktree change for a [`EditEntry`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Added,
    Deleted,
    Modified,
}

impl EditKind {
    pub fn sigil(self) -> &'static str {
        match self {
            EditKind::Added => "A",
            EditKind::Deleted => "D",
            EditKind::Modified => "M",
        }
    }
}

/// A file changed locally in the PR worktree (vs the checked-out PR head).
#[derive(Clone, Debug)]
pub struct EditEntry {
    pub path: String,
    pub kind: EditKind,
}

/// How much of a locally-changed file sits in the git index — the [4] pane's
/// equivalent of the Files pane's "viewed" mark.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StageState {
    Unstaged,
    /// Some hunks staged, some not (the [0] pane splits into two columns).
    Partial,
    Staged,
}

impl StageState {
    pub fn mark(self) -> &'static str {
        match self {
            StageState::Unstaged => " ",
            StageState::Partial => "~",
            StageState::Staged => "✔",
        }
    }
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
    Edits,
    Pending,
    Diff,
}

/// Tab-cycle order (matches top-to-bottom pane layout, diff last).
pub const FOCUS_ORDER: [Focus; 6] =
    [Focus::Prs, Focus::Commits, Focus::Files, Focus::Edits, Focus::Pending, Focus::Diff];

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
            '4' => Some(Focus::Edits),
            '5' => Some(Focus::Pending),
            _ => None,
        }
    }
}

/// The submit dialog's choices, in display order. `CLAUDE` hands the comments to
/// a local Claude; the others post to the PR and submit the review with that event.
pub const SUBMIT_CHOICES: [(&str, &str); 4] = [
    ("CLAUDE", "Send to Claude"),
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
    /// Commit message for the pending worktree edits (commit + push).
    CommitMsg { ta: TextArea, kind: CommitKind },
    /// A commit's hooks, running: their output as it arrives.
    ///
    /// Closes itself when they pass — there is nothing to read in a green run.
    /// A failure keeps it open, in red, holding the output that explains why.
    Hooks { title: String, lines: Vec<String>, failed: bool, scroll: usize },
    /// Free-text question to launch a Claude session about the selected hunk.
    Ask { ta: TextArea },
    /// A yes/no confirmation before a destructive action.
    Confirm { prompt: String, kind: ConfirmKind },
}

/// What a commit message overlay will produce.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommitKind {
    /// A new commit, running the repo's hooks.
    New,
    /// A new commit with the hooks skipped.
    NoVerify,
    /// Fold the staged work into HEAD, running the hooks.
    Amend,
}

impl CommitKind {
    pub fn runs_hooks(self) -> bool {
        !matches!(self, CommitKind::NoVerify)
    }

    /// The word for what is about to happen, for titles and statuses.
    pub fn verb(self) -> &'static str {
        match self {
            CommitKind::Amend => "Amend",
            _ => "Commit",
        }
    }
}

/// The action a [`Overlay::Confirm`] performs when accepted.
#[derive(Clone, Debug)]
pub enum ConfirmKind {
    /// Revert a local edit (discard uncommitted changes to a file).
    RevertEdit { path: String, added: bool },
    /// Push a branch whose history was rewritten, which the remote will only
    /// take as a force.
    ForcePush,
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
    /// The active PR is the locally checked-out one: reviewed in the main repo
    /// (no worktree), comments stored locally, editor is the main nvim.
    pub local_mode: bool,

    pub commits: Vec<Commit>,
    pub commit_selected: HashSet<String>,
    pub commit_idx: usize,
    pub commit_offset: usize,

    pub files: Vec<FileEntry>,
    /// The PR's own changed files (from GitHub); `files` is this plus any
    /// edit-only local files merged in for display.
    pub pr_files: Vec<FileEntry>,
    pub viewed_by_path: HashMap<String, bool>,
    /// In-flight optimistic viewed change: (paths, target value), so a failed
    /// mark can be reverted.
    pub viewed_inflight: Option<(Vec<String>, bool)>,
    pub collapsed_dirs: HashSet<String>,
    pub tree: Vec<TreeRow>,
    pub file_idx: usize,
    pub file_offset: usize,

    // Pending edits: local worktree changes vs the checked-out PR head.
    pub edit_files: Vec<EditEntry>,
    pub edit_collapsed: HashSet<String>,
    pub edit_tree: Vec<TreeRow>,
    pub edit_idx: usize,
    pub edit_offset: usize,
    /// Everything changed vs HEAD (staged + unstaged), shown whenever a file
    /// sits entirely on one side of the index.
    pub edit_diff_by_file: DiffMap,
    pub edit_info_by_file: InfoMap,
    pub edit_hunks_by_file: HunkMap,
    /// Worktree vs index (`git diff`) — the left column of a split local diff.
    pub unstaged_diff_by_file: DiffMap,
    pub unstaged_info_by_file: InfoMap,
    pub unstaged_hunks_by_file: HunkMap,
    /// Index vs HEAD (`git diff --cached`) — the right column of a split.
    pub staged_diff_by_file: DiffMap,
    pub staged_info_by_file: InfoMap,
    pub staged_hunks_by_file: HunkMap,
    /// In a split local diff, whether the staged (right) column has the cursor.
    pub staged_side: bool,
    /// `(scroll, hunk_idx)` of the *other* column of a split local diff; swapped
    /// with the live pair when switching sides.
    pub alt_diff_view: (usize, usize),
    pub edit_diff_scroll: usize,
    /// When set, the [0] pane shows this file's *local* diff (from [4]) with hunk
    /// navigation instead of the PR review diff.
    pub local_diff_path: Option<String>,
    /// Path → local change kind, for coloring edited files in [3] and [4].
    pub edit_kind_by_path: HashMap<String, EditKind>,

    /// First `g` of a pending `gg` (jump-to-top) chord, in the tree panes.
    pub pending_g: bool,

    /// Per-worktree Neovim sockets we launched (value = confirmed-alive at least
    /// once), and whether we created the Hyprland window group ourselves — both
    /// used to tear things down on exit / when the editor is closed.
    pub worktree_editors: HashMap<String, bool>,
    /// A file another tool asked us to show, held until the edit list has
    /// loaded and we can actually select it.
    pub pending_open_file: Option<String>,
    /// A commit another tool asked us to review, held until a PR is loaded.
    pub pending_open_commit: Option<String>,
    /// Set by an amend: the branch's history was rewritten, so the next push
    /// has to be a lease-force or the remote refuses it as non-fast-forward.
    pub amended: bool,
    pub entered_group: bool,

    pub diff_by_file: DiffMap,
    pub info_by_file: InfoMap,
    pub hunks_by_file: HunkMap,
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
