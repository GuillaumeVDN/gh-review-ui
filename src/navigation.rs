//! Cursor / hunk / selection logic over [`State`] (pure).
//!
//! A "hunk" here is a diff block (a contiguous run of +/- lines), so a range
//! starts on a real changed line. Helpers still tolerate a header at the start
//! (they skip `(None, None)` rows), so they work either way.

use crate::models::{
    DiffMap, HunkMap, InfoMap, LineInfo, PendingComment, Range, ReviewThread, StageState, State,
    TreeRow,
};

/// The three per-file maps backing `path`'s currently-shown diff: the PR review
/// diff, or the local one, which is the whole change against HEAD.
pub fn source_maps<'a>(st: &'a State, path: &str) -> (&'a DiffMap, &'a InfoMap, &'a HunkMap) {
    match is_local_diff(st, path) {
        false => (&st.diff_by_file, &st.info_by_file, &st.hunks_by_file),
        true => (&st.edit_diff_by_file, &st.edit_info_by_file, &st.edit_hunks_by_file),
    }
}

/// The diff lines backing `path`'s currently-shown diff (local edits or PR).
pub fn diff_lines<'a>(st: &'a State, path: &str) -> Option<&'a Vec<String>> {
    source_maps(st, path).0.get(path)
}

/// The per-line info backing `path`'s currently-shown diff (local edits or PR).
pub fn info_lines<'a>(st: &'a State, path: &str) -> Option<&'a Vec<LineInfo>> {
    source_maps(st, path).1.get(path)
}

/// How much of `path`'s local change is staged.
pub fn stage_state(st: &State, path: &str) -> StageState {
    match (st.staged_paths.contains(path), st.unstaged_paths.contains(path)) {
        (true, true) => StageState::Partial,
        (true, false) => StageState::Staged,
        _ => StageState::Unstaged,
    }
}

/// Pending-comment indices in the Pending pane's display (tree) order: grouped
/// by file, files in the same order as the rendered tree.
pub fn pending_order(st: &State) -> Vec<usize> {
    let mut paths: Vec<String> = st.pending.iter().map(|c| c.path.clone()).collect();
    paths.sort();
    paths.dedup();
    let tree = crate::tree::build_tree_from_paths(&paths, &std::collections::HashSet::new());
    let mut order = Vec::new();
    for row in tree {
        if let TreeRow::File { index, .. } = row {
            let path = &paths[index];
            for (ci, c) in st.pending.iter().enumerate() {
                if &c.path == path {
                    order.push(ci);
                }
            }
        }
    }
    order
}

pub fn cur_file_path(st: &State) -> Option<String> {
    match st.tree.get(st.file_idx) {
        Some(TreeRow::File { index, .. }) => Some(st.files[*index].path.clone()),
        _ => None,
    }
}

/// Whether the [0] pane is currently showing `path`'s *local* diff (from [4])
/// rather than the PR review diff.
pub fn is_local_diff(st: &State, path: &str) -> bool {
    st.local_diff_path.as_deref() == Some(path)
        || (!st.diff_by_file.contains_key(path) && st.edit_diff_by_file.contains_key(path))
}

/// The path whose diff the [0] pane shows: the pinned local file, else the
/// selected Files-pane file.
pub fn diff_path(st: &State) -> Option<String> {
    st.local_diff_path.clone().or_else(|| cur_file_path(st))
}

pub fn current_hunk_range(st: &State, path: &str) -> Option<Range> {
    let hunks = source_maps(st, path).2.get(path)?;
    if hunks.is_empty() {
        return None;
    }
    let idx = st.diff_hunk_idx.min(hunks.len() - 1);
    Some(hunks[idx])
}

/// `(line_no, side)` a comment on diff-line `idx` attaches to.
pub fn line_target(st: &State, path: &str, idx: usize) -> Option<(i64, String)> {
    let info = info_lines(st, path)?;
    let (old, new) = *info.get(idx)?;
    match (old, new) {
        (None, Some(n)) => Some((n, "RIGHT".into())),   // added
        (Some(o), None) => Some((o, "LEFT".into())),    // deleted
        (Some(_), Some(n)) => Some((n, "RIGHT".into())), // context → new side
        (None, None) => None,
    }
}

/// Diff-line indices in the current hunk that a comment can attach to.
pub fn hunk_line_indices(st: &State, path: &str) -> Vec<usize> {
    let Some((s, e)) = current_hunk_range(st, path) else {
        return Vec::new();
    };
    let empty = Vec::new();
    let info = info_lines(st, path).unwrap_or(&empty);
    (s..e)
        .filter(|&i| info.get(i).map_or(false, |&t| t != (None, None)))
        .collect()
}

/// Diff-line indices a comment can attach to in the current `@@` section
/// (context lines included, not just the changed block) — so comments can land
/// on the surrounding context.
pub fn hunk_comment_indices(st: &State, path: &str) -> Vec<usize> {
    let empty = Vec::new();
    let lines = diff_lines(st, path).unwrap_or(&empty);
    let empty_info = Vec::new();
    let info = info_lines(st, path).unwrap_or(&empty_info);
    let Some((s, e)) = current_hunk_range(st, path) else { return Vec::new() };
    // Expand from the change block out to the enclosing @@ section boundaries.
    let mut lo = s;
    while lo > 0 && !lines[lo - 1].starts_with("@@") {
        lo -= 1;
    }
    let mut hi = e;
    while hi < lines.len() && !lines[hi].starts_with("@@") {
        hi += 1;
    }
    (lo..hi).filter(|&i| info.get(i).map_or(false, |&t| t != (None, None))).collect()
}

/// Diff-line index of the first added/deleted line in the current hunk.
pub fn first_change_index(st: &State, path: &str) -> Option<usize> {
    if let Some((s, e)) = current_hunk_range(st, path) {
        if let Some(info) = info_lines(st, path) {
            for i in s..e {
                if let Some(&(old, new)) = info.get(i) {
                    if old.is_some() != new.is_some() {
                        return Some(i);
                    }
                }
            }
        }
    }
    hunk_line_indices(st, path).first().copied()
}

pub fn scroll_diff(st: &mut State, delta: i64) {
    let s = st.diff_scroll as i64 + delta;
    st.diff_scroll = s.max(0) as usize;
}

/// Move the hunk selection. Scrolling to keep it visible is a render concern.
pub fn jump_hunk(st: &mut State, direction: i64) {
    let Some(path) = diff_path(st) else { return };
    let len = source_maps(st, &path).2.get(&path).map_or(0, |h| h.len());
    if len == 0 {
        return;
    }
    let idx = (st.diff_hunk_idx as i64 + direction).clamp(0, len as i64 - 1);
    st.diff_hunk_idx = idx as usize;
    st.diff_reveal_pending = true;
}

/// A place `j`/`k` stops on in the diff pane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stop {
    /// A change block, by its index in the file's hunk list.
    Block(usize),
    /// A pending comment of ours, by its index in `st.pending`.
    Pending(usize),
    /// An unresolved review thread, by its index in `st.threads`.
    Thread(usize),
}

/// Diff-line index a comment anchored on `line`/`side` is drawn under.
pub fn anchor_index(info: &[LineInfo], line: Option<i64>, side: &str) -> Option<usize> {
    let line = line?;
    info.iter().position(|&(old, new)| if side == "LEFT" { old == Some(line) } else { new == Some(line) })
}

/// Where a thread hangs in `path`'s diff. `None` when the diff on screen has no
/// such line, which is where an outdated thread ends up.
pub fn thread_anchor(st: &State, path: &str, t: &ReviewThread) -> Option<usize> {
    anchor_index(info_lines(st, path)?, t.line, &t.side)
}

/// The stops of `path`'s diff, in the order they are drawn: the change blocks
/// plus the comments that hang under a line. A thread the diff cannot place
/// sits at the top of the file.
///
/// A local diff carries only blocks: its line numbers are the worktree's, and
/// the comments are answers to the PR.
pub fn diff_stops(st: &State, path: &str) -> Vec<Stop> {
    let empty = Vec::new();
    let blocks = source_maps(st, path).2.get(path).unwrap_or(&empty);
    // (diff line, rank, index): the rank orders what shares a line, a block
    // first and then the comments drawn under it.
    let mut keyed: Vec<((usize, u8, usize), Stop)> = blocks
        .iter()
        .enumerate()
        .map(|(i, &(s, _))| ((s, 1, i), Stop::Block(i)))
        .collect();
    if !is_local_diff(st, path) {
        let info = info_lines(st, path);
        for (i, c) in st.pending.iter().enumerate().filter(|(_, c)| c.path == path) {
            let at = info.and_then(|inf| anchor_index(inf, Some(c.line), &c.side));
            if let Some(at) = at {
                keyed.push(((at, 2, i), Stop::Pending(i)));
            }
        }
        for (i, t) in st.threads.iter().enumerate().filter(|(_, t)| t.path == path) {
            match info.and_then(|inf| anchor_index(inf, t.line, &t.side)) {
                Some(at) => keyed.push(((at, 3, i), Stop::Thread(i))),
                None => keyed.push(((0, 0, i), Stop::Thread(i))),
            }
        }
    }
    keyed.sort_by_key(|(k, _)| *k);
    keyed.into_iter().map(|(_, s)| s).collect()
}

/// The change block a stop belongs to: the one it is, or the last one above it.
fn block_of(stops: &[Stop], idx: usize) -> Option<usize> {
    if let Some(Stop::Block(b)) = stops.get(idx) {
        return Some(*b);
    }
    stops[..idx].iter().rev().find_map(|s| match s {
        Stop::Block(b) => Some(*b),
        _ => None,
    })
}

/// The stop the diff pane sits on.
pub fn focused_stop(st: &State) -> Option<Stop> {
    let path = diff_path(st)?;
    diff_stops(st, &path).get(st.diff_stop_idx).copied()
}

/// Move from stop to stop: change blocks and inline comments alike. Scrolling
/// to keep the stop visible is a render concern.
pub fn jump_stop(st: &mut State, direction: i64) {
    let Some(path) = diff_path(st) else { return };
    let stops = diff_stops(st, &path);
    if stops.is_empty() {
        return;
    }
    let idx = (st.diff_stop_idx as i64 + direction).clamp(0, stops.len() as i64 - 1) as usize;
    st.diff_stop_idx = idx;
    // `c`, `e` and the picker all work on a block, so one stays selected.
    if let Some(b) = block_of(&stops, idx) {
        st.diff_hunk_idx = b;
    }
    st.diff_reveal_pending = true;
}

/// The diff-line range to scroll into view for the current stop: the block, or
/// the anchored line plus room for the comment under it.
pub fn stop_reveal(st: &State, path: &str) -> Option<Range> {
    let stops = diff_stops(st, path);
    let at = match stops.get(st.diff_stop_idx)? {
        Stop::Block(_) => return current_hunk_range(st, path),
        Stop::Pending(i) => {
            let c = st.pending.get(*i)?;
            anchor_index(info_lines(st, path)?, Some(c.line), &c.side)?
        }
        Stop::Thread(i) => {
            let t = st.threads.get(*i)?;
            thread_anchor(st, path, t).unwrap_or(0)
        }
    };
    Some((at, (at + 3).min(diff_lines(st, path).map_or(at + 1, Vec::len))))
}

/// New-file line to open in the editor for the selected hunk.
pub fn current_hunk_editor_line(st: &State, path: &str) -> i64 {
    let info_map = source_maps(st, path).1;
    if let Some((s, e)) = current_hunk_range(st, path) {
        if let Some(info) = info_map.get(path) {
            for i in s..e {
                if let Some(&(old, new)) = info.get(i) {
                    if let (None, Some(n)) = (old, new) {
                        return n; // first added line
                    }
                }
            }
            // pure-deletion block: nearest new-side line after, then before
            for i in s..info.len() {
                if let Some(n) = info[i].1 {
                    return n;
                }
            }
            for i in (0..=s.min(info.len().saturating_sub(1))).rev() {
                if let Some(n) = info[i].1 {
                    return n;
                }
            }
        }
    }
    1
}

/// `(hunk_lines, target_offset)` for the hunk a pending comment anchors to.
pub fn hunk_for_comment(st: &State, c: &PendingComment) -> (Vec<String>, Option<usize>) {
    let empty_lines = Vec::new();
    let lines = st.diff_by_file.get(&c.path).unwrap_or(&empty_lines);
    let empty_info = Vec::new();
    let info = st.info_by_file.get(&c.path).unwrap_or(&empty_info);
    let empty_hunks = Vec::new();
    let hunks = st.hunks_by_file.get(&c.path).unwrap_or(&empty_hunks);

    let mut target = None;
    for (i, &(old, new)) in info.iter().enumerate() {
        let hit = if c.side == "LEFT" { old == Some(c.line) } else { new == Some(c.line) };
        if hit {
            target = Some(i);
            break;
        }
    }
    if let Some(t) = target {
        for &(s, e) in hunks {
            if s <= t && t < e {
                return (lines[s..e].to_vec(), Some(t - s));
            }
        }
    }
    (Vec::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FileEntry, PendingComment, ReviewThread, TreeRow};

    /// However much of a file is staged, its local diff is the whole change
    /// against HEAD.
    #[test]
    fn a_partly_staged_file_shows_its_change_against_head() {
        let mut st = State::default();
        let combined = vec!["@@ -1,2 +1,2 @@".to_string(), "-old".to_string(), "+new".to_string()];
        st.edit_diff_by_file.insert("f.rs".into(), combined.clone());
        st.local_diff_path = Some("f.rs".into());
        st.staged_paths.insert("f.rs".into());
        st.unstaged_paths.insert("f.rs".into());

        assert_eq!(stage_state(&st, "f.rs"), StageState::Partial);
        assert_eq!(diff_lines(&st, "f.rs"), Some(&combined));
        // The review diff of the same path is another thing entirely.
        st.diff_by_file.insert("f.rs".into(), vec!["@@ -1 +1 @@".to_string()]);
        st.local_diff_path = None;
        assert_eq!(diff_lines(&st, "f.rs").map(Vec::len), Some(1));
    }

    fn diff_state() -> State {
        let mut st = State::default();
        st.files = vec![FileEntry { path: "f.py".into(), viewed: false }];
        st.tree = vec![TreeRow::File { depth: 0, name: "f.py".into(), index: 0 }];
        st.file_idx = 0;
        let lines = vec![
            "@@ -1,2 +1,3 @@", " ctx", "+added", "+added2",
            "@@ -10,1 +11,2 @@", " keep", "-removed",
        ];
        st.diff_by_file.insert("f.py".into(), lines.iter().map(|s| s.to_string()).collect());
        st.info_by_file.insert(
            "f.py".into(),
            vec![
                (None, None), (Some(1), Some(1)), (None, Some(2)), (None, Some(3)),
                (None, None), (Some(11), Some(11)), (Some(12), None),
            ],
        );
        // manual @@-based ranges — helpers must still work via the (None,None) skip
        st.hunks_by_file.insert("f.py".into(), vec![(0, 4), (4, 7)]);
        st
    }

    #[test]
    fn local_diff_detection() {
        let mut st = State::default();
        st.diff_by_file.insert("pr.rs".into(), vec![]);
        st.edit_diff_by_file.insert("new.rs".into(), vec![]); // edit-only
        st.edit_diff_by_file.insert("pr.rs".into(), vec![]); // also has local edits
        assert!(!is_local_diff(&st, "pr.rs")); // a PR file, no pin → PR diff
        assert!(is_local_diff(&st, "new.rs")); // not in PR diff → local
        st.local_diff_path = Some("pr.rs".into());
        assert!(is_local_diff(&st, "pr.rs")); // pinned from [4] → local
    }

    #[test]
    fn current_hunk_tracks_index() {
        let mut st = diff_state();
        st.diff_hunk_idx = 0;
        assert_eq!(current_hunk_range(&st, "f.py"), Some((0, 4)));
        st.diff_hunk_idx = 1;
        st.diff_scroll = 0;
        assert_eq!(current_hunk_range(&st, "f.py"), Some((4, 7)));
    }

    #[test]
    fn jump_clamps() {
        let mut st = diff_state();
        st.diff_scroll = 7;
        jump_hunk(&mut st, 1);
        assert_eq!(st.diff_hunk_idx, 1);
        jump_hunk(&mut st, 1);
        assert_eq!(st.diff_hunk_idx, 1);
        jump_hunk(&mut st, -1);
        assert_eq!(st.diff_hunk_idx, 0);
        assert_eq!(st.diff_scroll, 7); // jump_hunk never scrolls
    }

    #[test]
    fn line_target_by_kind() {
        let st = diff_state();
        assert_eq!(line_target(&st, "f.py", 2), Some((2, "RIGHT".into())));
        assert_eq!(line_target(&st, "f.py", 6), Some((12, "LEFT".into())));
        assert_eq!(line_target(&st, "f.py", 1), Some((1, "RIGHT".into())));
        assert_eq!(line_target(&st, "f.py", 0), None);
    }

    #[test]
    fn indices_and_first_change() {
        let mut st = diff_state();
        st.diff_hunk_idx = 0;
        assert_eq!(hunk_line_indices(&st, "f.py"), vec![1, 2, 3]);
        assert_eq!(first_change_index(&st, "f.py"), Some(2));
        st.diff_hunk_idx = 1;
        assert_eq!(first_change_index(&st, "f.py"), Some(6));
    }

    #[test]
    fn editor_line() {
        let mut st = diff_state();
        st.diff_hunk_idx = 0;
        assert_eq!(current_hunk_editor_line(&st, "f.py"), 2);
        st.diff_hunk_idx = 1;
        assert_eq!(current_hunk_editor_line(&st, "f.py"), 11); // pure-deletion fallback
    }

    fn comment_on(path: &str, line: i64, side: &str) -> PendingComment {
        PendingComment {
            path: path.into(),
            body: "look".into(),
            line,
            side: side.into(),
            comment_id: "c1".into(),
            start_line: None,
            start_side: String::new(),
        }
    }

    fn comment(line: i64, side: &str) -> PendingComment {
        comment_on("f.py", line, side)
    }

    fn thread(line: Option<i64>, side: &str) -> ReviewThread {
        ReviewThread {
            id: "t1".into(),
            path: "f.py".into(),
            line,
            start_line: None,
            side: side.into(),
            start_side: side.into(),
            outdated: line.is_none(),
            original_line: Some(99),
            comments: Vec::new(),
        }
    }

    #[test]
    fn stops_interleave_comments_with_the_blocks() {
        let mut st = diff_state();
        // Anchored on the second added line, inside the first block.
        st.pending = vec![comment(3, "RIGHT")];
        // Anchored on the deleted line, the last row of the second block.
        st.threads = vec![thread(Some(12), "LEFT")];
        assert_eq!(
            diff_stops(&st, "f.py"),
            vec![Stop::Block(0), Stop::Pending(0), Stop::Block(1), Stop::Thread(0)],
        );
    }

    /// A comment on the context above a change block is read before it.
    #[test]
    fn a_comment_above_a_block_stops_before_it() {
        use crate::diff::{compute_hunks, parse_diff};
        let raw = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,3 @@\n ctx\n+added\n tail\n";
        let (lines, info) = parse_diff(raw);
        let mut st = State {
            files: vec![FileEntry { path: "f".into(), viewed: false }],
            tree: vec![TreeRow::File { depth: 0, name: "f".into(), index: 0 }],
            pending: vec![comment_on("f", 1, "RIGHT")],
            ..Default::default()
        };
        st.hunks_by_file.insert("f".into(), compute_hunks(&lines["f"]));
        st.diff_by_file = lines;
        st.info_by_file = info;
        assert_eq!(diff_stops(&st, "f"), vec![Stop::Pending(0), Stop::Block(0)]);
    }

    /// A LEFT comment reads the old numbers, a RIGHT one the new ones. The two
    /// name different rows of the same diff.
    #[test]
    fn the_side_picks_which_line_number_answers() {
        let st = diff_state();
        let info = info_lines(&st, "f.py").unwrap();
        assert_eq!(anchor_index(info, Some(12), "LEFT"), Some(6));
        assert_eq!(anchor_index(info, Some(12), "RIGHT"), None);
        assert_eq!(anchor_index(info, Some(11), "RIGHT"), Some(5));
        assert_eq!(anchor_index(info, None, "RIGHT"), None);
    }

    /// An outdated thread with no line left on the diff reads at the top.
    #[test]
    fn a_thread_without_a_line_stops_first() {
        let mut st = diff_state();
        st.threads = vec![thread(None, "RIGHT"), thread(Some(2), "RIGHT")];
        assert_eq!(
            diff_stops(&st, "f.py"),
            vec![Stop::Thread(0), Stop::Block(0), Stop::Thread(1), Stop::Block(1)],
        );
        assert!(thread_anchor(&st, "f.py", &st.threads[0]).is_none());
    }

    #[test]
    fn a_comment_stop_keeps_the_block_it_sits_in() {
        let mut st = diff_state();
        st.pending = vec![comment(3, "RIGHT")];
        jump_stop(&mut st, 1);
        assert_eq!(focused_stop(&st), Some(Stop::Pending(0)));
        assert_eq!(st.diff_hunk_idx, 0, "`c` comments on the enclosing block");
        jump_stop(&mut st, 1);
        assert_eq!(focused_stop(&st), Some(Stop::Block(1)));
        assert_eq!(st.diff_hunk_idx, 1);
        jump_stop(&mut st, 1);
        assert_eq!(st.diff_stop_idx, 2, "the last stop holds");
    }

    /// The local diff of a file is the worktree's lines, which the PR comments
    /// do not number.
    #[test]
    fn a_local_diff_stops_on_blocks_alone() {
        let mut st = diff_state();
        st.pending = vec![comment(3, "RIGHT")];
        st.edit_diff_by_file.insert("f.py".into(), st.diff_by_file["f.py"].clone());
        st.edit_info_by_file.insert("f.py".into(), st.info_by_file["f.py"].clone());
        st.edit_hunks_by_file.insert("f.py".into(), st.hunks_by_file["f.py"].clone());
        st.local_diff_path = Some("f.py".into());
        assert_eq!(diff_stops(&st, "f.py"), vec![Stop::Block(0), Stop::Block(1)]);
    }

    #[test]
    fn blocks_are_two_units() {
        use crate::diff::{compute_hunks, parse_diff};
        let raw = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,4 @@\n\
-test\n+test2\n context\n-test3\n+test4\n";
        let (lines, info) = parse_diff(raw);
        let mut st = State::default();
        st.files = vec![FileEntry { path: "f".into(), viewed: false }];
        st.tree = vec![TreeRow::File { depth: 0, name: "f".into(), index: 0 }];
        st.file_idx = 0;
        st.hunks_by_file.insert("f".into(), compute_hunks(&lines["f"]));
        st.diff_by_file = lines;
        st.info_by_file = info;
        assert_eq!(st.hunks_by_file["f"].len(), 2);
        let l = st.diff_by_file["f"].clone();
        let idx0: Vec<String> = hunk_line_indices(&st, "f").iter().map(|&i| l[i].clone()).collect();
        assert_eq!(idx0, ["-test", "+test2"]);
        jump_hunk(&mut st, 1);
        let idx1: Vec<String> = hunk_line_indices(&st, "f").iter().map(|&i| l[i].clone()).collect();
        assert_eq!(idx1, ["-test3", "+test4"]);
    }
}
