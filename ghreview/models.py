"""Domain dataclasses and the central application ``State``."""
from dataclasses import dataclass, field
from typing import Optional

# Focus targets. The integer order also drives Tab cycling; the number-key
# shortcuts are PRs [1], Commits [2], Pending [3], Files [4], Diff [0].
FOCUS_PRS, FOCUS_COMMITS, FOCUS_PENDING, FOCUS_FILES, FOCUS_DIFF = 0, 1, 2, 3, 4
N_PANES = 5


@dataclass
class PR:
    number: int
    title: str
    head: str
    author: str
    node_id: str = ""


@dataclass
class Commit:
    oid: str
    headline: str
    body: str = ""
    author: str = ""
    date: str = ""

    @property
    def short(self):
        return self.oid[:7]


@dataclass
class FileEntry:
    path: str
    viewed: bool


@dataclass
class PendingComment:
    path: str
    body: str
    line: int          # line number on the chosen side
    side: str          # "RIGHT" or "LEFT"
    comment_id: str = ""  # server node id once it exists in the pending review


@dataclass
class State:
    repo_owner: str = ""
    repo_name: str = ""
    repo_root: str = ""
    viewer: str = ""  # login of the authenticated user

    prs: list = field(default_factory=list)
    pr_idx: int = 0
    pr_view_offset: int = 0
    active_pr: Optional[PR] = None

    # Commits of the active PR + which are selected for review. Selection is a
    # contiguous range: the reviewed diff spans the earliest..latest selected
    # commit. All commits are selected by default (i.e. the whole PR).
    commits: list = field(default_factory=list)
    commit_selected: set = field(default_factory=set)  # selected oids
    commit_idx: int = 0
    commit_view_offset: int = 0

    files: list = field(default_factory=list)
    # Viewed state for every file in the PR, keyed by path. Kept separate from
    # ``files`` so it survives when the file list is filtered to a commit range.
    viewed_by_path: dict = field(default_factory=dict)
    collapsed_dirs: set = field(default_factory=set)
    tree: list = field(default_factory=list)
    file_idx: int = 0
    file_view_offset: int = 0

    diff_by_file: dict = field(default_factory=dict)
    info_by_file: dict = field(default_factory=dict)
    hunks_by_file: dict = field(default_factory=dict)
    diff_scroll: int = 0
    diff_hunk_idx: int = 0  # currently-selected hunk (independent of scroll)

    # PR details for the right pane when PRs is focused
    pr_details: dict = field(default_factory=dict)  # number -> data or None (loading)
    details_scroll: int = 0

    # Pending review comments (mirrors the server-side draft review)
    pending: list = field(default_factory=list)
    pending_idx: int = 0
    pending_view_offset: int = 0

    focus: int = FOCUS_PRS
    status: str = ""
    busy: set = field(default_factory=set)
    rects: dict = field(default_factory=dict)
