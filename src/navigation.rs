//! Cursor / hunk / selection logic over [`State`] (pure).
//!
//! A "hunk" here is a diff block (a contiguous run of +/- lines), so a range
//! starts on a real changed line. Helpers still tolerate a header at the start
//! (they skip `(None, None)` rows), so they work either way.

use crate::models::{PendingComment, Range, State, TreeRow};

pub fn cur_file_path(st: &State) -> Option<String> {
    match st.tree.get(st.file_idx) {
        Some(TreeRow::File { index, .. }) => Some(st.files[*index].path.clone()),
        _ => None,
    }
}

pub fn current_hunk_range(st: &State, path: &str) -> Option<Range> {
    let hunks = st.hunks_by_file.get(path)?;
    if hunks.is_empty() {
        return None;
    }
    let idx = st.diff_hunk_idx.min(hunks.len() - 1);
    Some(hunks[idx])
}

/// `(line_no, side)` a comment on diff-line `idx` attaches to.
pub fn line_target(st: &State, path: &str, idx: usize) -> Option<(i64, String)> {
    let info = st.info_by_file.get(path)?;
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
    let info = st.info_by_file.get(path).unwrap_or(&empty);
    (s..e)
        .filter(|&i| info.get(i).map_or(false, |&t| t != (None, None)))
        .collect()
}

/// Diff-line index of the first added/deleted line in the current hunk.
pub fn first_change_index(st: &State, path: &str) -> Option<usize> {
    if let Some((s, e)) = current_hunk_range(st, path) {
        if let Some(info) = st.info_by_file.get(path) {
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

/// `(hunk_idx, line_idx)` of the first commentable line after `after` (across
/// all hunks of `path`), or None if `after` was the last one.
pub fn next_commentable_after(st: &State, path: &str, after: usize) -> Option<(usize, usize)> {
    let hunks = st.hunks_by_file.get(path)?;
    let info = st.info_by_file.get(path)?;
    let mut best: Option<(usize, usize)> = None; // (line_idx, hunk_idx)
    for (hi, &(s, e)) in hunks.iter().enumerate() {
        for i in s..e {
            if i > after && info.get(i).map_or(false, |&t| t != (None, None)) {
                if best.map_or(true, |(bi, _)| i < bi) {
                    best = Some((i, hi));
                }
            }
        }
    }
    best.map(|(li, hi)| (hi, li))
}

pub fn scroll_diff(st: &mut State, delta: i64) {
    let s = st.diff_scroll as i64 + delta;
    st.diff_scroll = s.max(0) as usize;
}

/// Move the hunk selection. Scrolling to keep it visible is a render concern.
pub fn jump_hunk(st: &mut State, direction: i64) {
    let Some(path) = cur_file_path(st) else { return };
    let len = st.hunks_by_file.get(&path).map_or(0, |h| h.len());
    if len == 0 {
        return;
    }
    let idx = (st.diff_hunk_idx as i64 + direction).clamp(0, len as i64 - 1);
    st.diff_hunk_idx = idx as usize;
    st.diff_reveal_pending = true;
}

/// New-file line to open in the editor for the selected hunk.
pub fn current_hunk_editor_line(st: &State, path: &str) -> i64 {
    if let Some((s, e)) = current_hunk_range(st, path) {
        if let Some(info) = st.info_by_file.get(path) {
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
    use crate::models::{FileEntry, TreeRow};

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

    #[test]
    fn next_commentable() {
        let st = diff_state();
        // after +added (idx 2) → +added2 (idx 3) in hunk 0
        assert_eq!(next_commentable_after(&st, "f.py", 2), Some((0, 3)));
        // after +added2 (idx 3) → next hunk's first commentable line (keep, idx 5)
        assert_eq!(next_commentable_after(&st, "f.py", 3), Some((1, 5)));
        // after the last line → nothing
        assert_eq!(next_commentable_after(&st, "f.py", 6), None);
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
