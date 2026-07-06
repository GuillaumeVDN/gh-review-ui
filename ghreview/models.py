"""Domain dataclasses and the central application ``State``."""
from dataclasses import dataclass, field
from typing import Optional

# Focus targets, matched to their number-key shortcut where handy.
FOCUS_PRS, FOCUS_PENDING, FOCUS_FILES, FOCUS_DIFF = 0, 1, 2, 3


@dataclass
class PR:
    number: int
    title: str
    head: str
    author: str
    node_id: str = ""


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

    files: list = field(default_factory=list)
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
