//! File-tree building and folding for the Files pane (pure).

use std::collections::{HashMap, HashSet};

use crate::models::{FileEntry, StageState, State, TreeRow};

enum Node {
    Dir(HashMap<String, Node>),
    File(usize),
}

/// Build the display rows (dirs before files, case-insensitive) for `files`.
pub fn build_tree(files: &[FileEntry], collapsed: &HashSet<String>) -> Vec<TreeRow> {
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    build_tree_from_paths(&paths, collapsed)
}

/// Build the display rows for a bare list of paths; `TreeRow::File.index` refers
/// to the position in `paths`. Shared by the Files and Pending-edits panes.
pub fn build_tree_from_paths(paths: &[String], collapsed: &HashSet<String>) -> Vec<TreeRow> {
    let mut root = Node::Dir(HashMap::new());
    for (i, path) in paths.iter().enumerate() {
        let parts: Vec<&str> = path.split('/').collect();
        let mut node = &mut root;
        for p in &parts[..parts.len() - 1] {
            let map = match node {
                Node::Dir(m) => m,
                Node::File(_) => break,
            };
            node = map
                .entry((*p).to_string())
                .or_insert_with(|| Node::Dir(HashMap::new()));
        }
        if let Node::Dir(m) = node {
            m.insert(parts[parts.len() - 1].to_string(), Node::File(i));
        }
    }
    let mut out = Vec::new();
    walk(&root, "", 0, collapsed, &mut out);
    out
}

fn walk(node: &Node, prefix: &str, depth: usize, collapsed: &HashSet<String>, out: &mut Vec<TreeRow>) {
    let map = match node {
        Node::Dir(m) => m,
        Node::File(_) => return,
    };
    let mut entries: Vec<(&String, &Node)> = map.iter().collect();
    entries.sort_by(|a, b| {
        let ka = (matches!(a.1, Node::File(_)), a.0.to_lowercase());
        let kb = (matches!(b.1, Node::File(_)), b.0.to_lowercase());
        ka.cmp(&kb)
    });
    for (name, child) in entries {
        match child {
            Node::Dir(_) => {
                let path = format!("{prefix}{name}");
                let is_collapsed = collapsed.contains(&path);
                out.push(TreeRow::Dir {
                    depth,
                    name: name.clone(),
                    path: path.clone(),
                    collapsed: is_collapsed,
                });
                if !is_collapsed {
                    walk(child, &format!("{path}/"), depth + 1, collapsed, out);
                }
            }
            Node::File(idx) => out.push(TreeRow::File {
                depth,
                name: name.clone(),
                index: *idx,
            }),
        }
    }
}

pub fn rebuild(st: &mut State) {
    st.tree = build_tree(&st.files, &st.collapsed_dirs);
    if st.file_idx >= st.tree.len() {
        st.file_idx = st.tree.len().saturating_sub(1);
    }
}

/// Rebuild the Pending-edits tree from the current `edit_files`.
pub fn rebuild_edits(st: &mut State) {
    let paths: Vec<String> = st.edit_files.iter().map(|e| e.path.clone()).collect();
    st.edit_tree = build_tree_from_paths(&paths, &st.edit_collapsed);
    if st.edit_idx >= st.edit_tree.len() {
        st.edit_idx = st.edit_tree.len().saturating_sub(1);
    }
}

/// Indices into `st.files` of the files under `dir_path`.
pub fn files_under_dir(st: &State, dir_path: &str) -> Vec<usize> {
    let prefix = format!("{dir_path}/");
    st.files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.path.starts_with(&prefix))
        .map(|(i, _)| i)
        .collect()
}

/// Collapse every folder whose files are all viewed. Returns the count folded.
pub fn fold_viewed_dirs(st: &mut State) -> usize {
    let mut dirs: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, f) in st.files.iter().enumerate() {
        let parts: Vec<&str> = f.path.split('/').collect();
        for d in 1..parts.len() {
            dirs.entry(parts[..d].join("/")).or_default().push(i);
        }
    }
    let mut folded = 0;
    for (path, idxs) in dirs {
        let all_viewed = !idxs.is_empty() && idxs.iter().all(|&i| st.files[i].viewed);
        if all_viewed && !st.collapsed_dirs.contains(&path) {
            st.collapsed_dirs.insert(path);
            folded += 1;
        }
    }
    rebuild(st);
    folded
}

/// Collapse every folder of the Pending-edits tree whose files are all staged.
///
/// The `[4]` pane's fold, with "fully staged" standing in for "viewed" — it is
/// the same statement about a file: there is nothing left to do here.
pub fn fold_staged_dirs(st: &mut State) -> usize {
    let mut dirs: HashMap<String, Vec<String>> = HashMap::new();
    for e in &st.edit_files {
        let parts: Vec<&str> = e.path.split('/').collect();
        for d in 1..parts.len() {
            dirs.entry(parts[..d].join("/")).or_default().push(e.path.clone());
        }
    }
    // Decided before collapsing: `stage_state` borrows the state we are about
    // to write to.
    let to_fold: Vec<String> = dirs
        .into_iter()
        .filter(|(path, paths)| {
            !paths.is_empty()
                && !st.edit_collapsed.contains(path)
                && paths
                    .iter()
                    .all(|p| crate::navigation::stage_state(st, p) == StageState::Staged)
        })
        .map(|(path, _)| path)
        .collect();
    let folded = to_fold.len();
    for path in to_fold {
        st.edit_collapsed.insert(path);
    }
    rebuild_edits(st);
    folded
}

/// Tree index of the first unstaged file within `dir_path`'s subtree, if any.
pub fn first_unstaged_in_dir(st: &State, dir_path: &str) -> Option<usize> {
    let prefix = format!("{dir_path}/");
    st.edit_tree.iter().position(|row| match row {
        TreeRow::File { index, .. } => st
            .edit_files
            .get(*index)
            .is_some_and(|e| {
                e.path.starts_with(&prefix)
                    && crate::navigation::stage_state(st, &e.path) != StageState::Staged
            }),
        _ => false,
    })
}

/// Indices into `st.edit_files` of the files under `dir_path`.
pub fn edit_files_under_dir(st: &State, dir_path: &str) -> Vec<usize> {
    let prefix = format!("{dir_path}/");
    st.edit_files
        .iter()
        .enumerate()
        .filter(|(_, e)| e.path.starts_with(&prefix))
        .map(|(i, _)| i)
        .collect()
}

/// Tree index of the first unviewed file, if any.
pub fn first_unviewed_index(st: &State) -> Option<usize> {
    st.tree.iter().position(|row| match row {
        TreeRow::File { index, .. } => !st.files[*index].viewed,
        _ => false,
    })
}

/// Tree index of the first unviewed file within `dir_path`'s subtree, if any.
pub fn first_unviewed_in_dir(st: &State, dir_path: &str) -> Option<usize> {
    let prefix = format!("{dir_path}/");
    st.tree.iter().position(|row| match row {
        TreeRow::File { index, .. } => {
            !st.files[*index].viewed && st.files[*index].path.starts_with(&prefix)
        }
        _ => false,
    })
}

/// Tree index of the "next" unviewed file relative to `anchor` (a file index into
/// `st.files`, typically the file the cursor sat on). Prefers the closest unviewed
/// file whose display order is *after* the anchor; if none remain after it, falls
/// back to the closest one *before* it. With no anchor, returns the first unviewed
/// file. The anchor itself is never returned.
pub fn next_unviewed_index(st: &State, anchor: Option<usize>) -> Option<usize> {
    let paths: Vec<String> = st.files.iter().map(|f| f.path.clone()).collect();
    let done: Vec<bool> = st.files.iter().map(|f| f.viewed).collect();
    next_pending_index(&st.tree, &paths, &done, anchor)
}

/// The same walk over the Pending-edits tree, where "done" is a fully staged
/// file rather than a viewed one.
pub fn next_unstaged_index(st: &State, anchor: Option<usize>) -> Option<usize> {
    let paths: Vec<String> = st.edit_files.iter().map(|e| e.path.clone()).collect();
    let done: Vec<bool> = paths
        .iter()
        .map(|p| crate::navigation::stage_state(st, p) == StageState::Staged)
        .collect();
    next_pending_index(&st.edit_tree, &paths, &done, anchor)
}

/// Shared by both panes: the next row still to do, given which are done.
///
/// `tree` is the rows as currently displayed (so a collapsed row is not a
/// destination), `paths` and `done` are indexed by the file index the rows
/// carry, and `anchor` is where the cursor sat.
fn next_pending_index(
    tree: &[TreeRow],
    paths: &[String],
    done: &[bool],
    anchor: Option<usize>,
) -> Option<usize> {
    // Stable display order over file indices, independent of the current collapse
    // state, so a folded-away anchor still has a well-defined position.
    let mut rank = vec![0usize; paths.len()];
    for (r, row) in build_tree_from_paths(paths, &HashSet::new()).iter().enumerate() {
        if let TreeRow::File { index, .. } = row {
            rank[*index] = r;
        }
    }
    let anchor_rank = anchor.map(|fi| rank[fi]);

    let mut after: Option<(usize, usize)> = None; // (rank, tree_pos): smallest rank after anchor
    let mut before: Option<(usize, usize)> = None; // (rank, tree_pos): largest rank before anchor
    for (ti, row) in tree.iter().enumerate() {
        let TreeRow::File { index, .. } = row else { continue };
        if done[*index] {
            continue;
        }
        let r = rank[*index];
        match anchor_rank {
            Some(a) if r > a => {
                if after.is_none_or(|(ar, _)| r < ar) {
                    after = Some((r, ti));
                }
            }
            Some(a) if r < a => {
                if before.is_none_or(|(br, _)| r > br) {
                    before = Some((r, ti));
                }
            }
            Some(_) => {} // r == a: the anchor itself, skip
            None => {
                if after.is_none_or(|(ar, _)| r < ar) {
                    after = Some((r, ti));
                }
            }
        }
    }
    after.or(before).map(|(_, ti)| ti)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[(&str, bool)]) -> Vec<FileEntry> {
        paths
            .iter()
            .map(|(p, v)| FileEntry { path: p.to_string(), viewed: *v })
            .collect()
    }

    /// The `[4]` pane folds by "fully staged" where `[3]` folds by "viewed".
    /// A folder with anything left to stage must stay open.
    #[test]
    fn only_wholly_staged_folders_fold() {
        let mut st = State::default();
        st.edit_files = ["src/a.rs", "src/b.rs", "docs/x.md"]
            .iter()
            .map(|p| crate::models::EditEntry {
                path: p.to_string(),
                kind: crate::models::EditKind::Modified,
            })
            .collect();
        // Staged means: present in the staged diff and absent from the unstaged one.
        for p in ["src/a.rs", "docs/x.md"] {
            st.staged_diff_by_file.insert(p.to_string(), Vec::new());
        }
        st.unstaged_diff_by_file.insert("src/b.rs".to_string(), Vec::new());
        rebuild_edits(&mut st);

        assert_eq!(fold_staged_dirs(&mut st), 1);
        assert!(st.edit_collapsed.contains("docs"), "everything in it is staged");
        assert!(!st.edit_collapsed.contains("src"), "b.rs is still unstaged");

        // Folding again finds nothing new rather than counting the same one twice.
        assert_eq!(fold_staged_dirs(&mut st), 0);
    }

    /// A partially staged file is not done, so its folder stays open.
    #[test]
    fn a_partly_staged_file_does_not_count_as_done() {
        let mut st = State::default();
        st.edit_files = vec![crate::models::EditEntry {
            path: "src/a.rs".into(),
            kind: crate::models::EditKind::Modified,
        }];
        st.staged_diff_by_file.insert("src/a.rs".to_string(), Vec::new());
        st.unstaged_diff_by_file.insert("src/a.rs".to_string(), Vec::new());
        rebuild_edits(&mut st);

        assert_eq!(fold_staged_dirs(&mut st), 0);
        assert!(next_unstaged_index(&st, None).is_some(), "it is still to do");
    }

    #[test]
    fn the_next_unstaged_file_is_the_one_after_the_cursor() {
        let mut st = State::default();
        st.edit_files = ["a.rs", "b.rs", "c.rs"]
            .iter()
            .map(|p| crate::models::EditEntry {
                path: p.to_string(),
                kind: crate::models::EditKind::Modified,
            })
            .collect();
        st.staged_diff_by_file.insert("b.rs".to_string(), Vec::new());
        rebuild_edits(&mut st);

        // From a.rs: b.rs is done, so c.rs is next.
        let ti = next_unstaged_index(&st, Some(0)).expect("something left");
        assert!(matches!(st.edit_tree[ti], TreeRow::File { index: 2, .. }));
        // From c.rs there is nothing after, so it falls back to a.rs before it.
        let ti = next_unstaged_index(&st, Some(2)).expect("something left");
        assert!(matches!(st.edit_tree[ti], TreeRow::File { index: 0, .. }));
    }

    #[test]
    fn nothing_is_left_when_every_file_is_staged() {
        let mut st = State::default();
        st.edit_files = vec![crate::models::EditEntry {
            path: "a.rs".into(),
            kind: crate::models::EditKind::Modified,
        }];
        st.staged_diff_by_file.insert("a.rs".to_string(), Vec::new());
        rebuild_edits(&mut st);
        assert!(next_unstaged_index(&st, None).is_none());
    }

    #[test]
    fn groups_dirs_before_files_sorted() {
        let f = files(&[("src/z.py", false), ("src/a.py", false), ("readme.md", false)]);
        let tree = build_tree(&f, &HashSet::new());
        // src (dir) before readme.md (file)
        assert!(matches!(&tree[0], TreeRow::Dir { name, .. } if name == "src"));
        let names: Vec<&str> = tree
            .iter()
            .filter_map(|r| match r {
                TreeRow::File { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        // a.py before z.py; readme.md last
        assert_eq!(names, ["a.py", "z.py", "readme.md"]);
    }

    #[test]
    fn collapsed_dir_hides_children() {
        let f = files(&[("src/a.py", false), ("src/b.py", false)]);
        let full = build_tree(&f, &HashSet::new());
        let mut c = HashSet::new();
        c.insert("src".to_string());
        let collapsed = build_tree(&f, &c);
        assert!(collapsed.len() < full.len());
        assert!(collapsed.iter().all(|r| !matches!(r, TreeRow::File { .. })));
    }

    #[test]
    fn fold_only_fully_viewed() {
        let mut st = State::default();
        st.files = files(&[
            ("all/a.py", true),
            ("all/b.py", true),
            ("mix/a.py", true),
            ("mix/b.py", false),
        ]);
        rebuild(&mut st);
        let folded = fold_viewed_dirs(&mut st);
        assert!(st.collapsed_dirs.contains("all"));
        assert!(!st.collapsed_dirs.contains("mix"));
        assert_eq!(folded, 1);
    }

    #[test]
    fn first_unviewed() {
        let mut st = State::default();
        st.files = files(&[("a.py", true), ("b.py", false)]);
        rebuild(&mut st);
        let ti = first_unviewed_index(&st).unwrap();
        assert!(matches!(&st.tree[ti], TreeRow::File { index, .. } if st.files[*index].path == "b.py"));
    }

    /// Given a display order a,b,c,d,e, an anchor of `c` should jump to the first
    /// unviewed file *after* c, not back to the first unviewed one at the top.
    fn file_at<'a>(st: &'a State, ti: usize) -> &'a str {
        match &st.tree[ti] {
            TreeRow::File { index, .. } => &st.files[*index].path,
            _ => panic!("row {ti} is not a file"),
        }
    }

    fn idx_of(st: &State, path: &str) -> usize {
        st.files.iter().position(|f| f.path == path).unwrap()
    }

    #[test]
    fn next_unviewed_prefers_after_anchor() {
        let mut st = State::default();
        // a and c are unviewed and sit before/after the anchor d; e is unviewed after.
        st.files = files(&[
            ("a.py", false),
            ("b.py", true),
            ("c.py", true),
            ("d.py", true),
            ("e.py", false),
        ]);
        rebuild(&mut st);
        let ti = next_unviewed_index(&st, Some(idx_of(&st, "d.py"))).unwrap();
        assert_eq!(file_at(&st, ti), "e.py");
    }

    #[test]
    fn next_unviewed_falls_back_to_closest_before() {
        let mut st = State::default();
        // Nothing unviewed after the anchor e; closest unviewed before is c (not a).
        st.files = files(&[
            ("a.py", false),
            ("b.py", true),
            ("c.py", false),
            ("d.py", true),
            ("e.py", true),
        ]);
        rebuild(&mut st);
        let ti = next_unviewed_index(&st, Some(idx_of(&st, "e.py"))).unwrap();
        assert_eq!(file_at(&st, ti), "c.py");
    }

    #[test]
    fn next_unviewed_without_anchor_is_first() {
        let mut st = State::default();
        st.files = files(&[("a.py", true), ("b.py", false), ("c.py", false)]);
        rebuild(&mut st);
        let ti = next_unviewed_index(&st, None).unwrap();
        assert_eq!(file_at(&st, ti), "b.py");
    }

    #[test]
    fn first_unviewed_in_dir_dives_into_subtree() {
        let mut st = State::default();
        // Cursor on "src": jump to its first unviewed file (b.py), not a.py above it.
        st.files = files(&[
            ("a.py", false),
            ("src/a.py", true),
            ("src/b.py", false),
            ("src/c.py", false),
        ]);
        rebuild(&mut st);
        let ti = first_unviewed_in_dir(&st, "src").unwrap();
        assert_eq!(file_at(&st, ti), "src/b.py");
    }

    #[test]
    fn first_unviewed_in_dir_none_when_all_viewed() {
        let mut st = State::default();
        st.files = files(&[("src/a.py", true), ("src/b.py", true)]);
        rebuild(&mut st);
        assert!(first_unviewed_in_dir(&st, "src").is_none());
    }

    #[test]
    fn next_unviewed_survives_folded_anchor() {
        let mut st = State::default();
        // The anchor dir "seen" is fully viewed and gets folded; the jump target
        // must still be computed relative to where it sat in display order.
        st.files = files(&[
            ("aaa/x.py", false),
            ("seen/a.py", true),
            ("seen/b.py", true),
            ("zzz/y.py", false),
        ]);
        rebuild(&mut st);
        let anchor = idx_of(&st, "seen/a.py");
        fold_viewed_dirs(&mut st);
        assert!(st.collapsed_dirs.contains("seen"));
        let ti = next_unviewed_index(&st, Some(anchor)).unwrap();
        assert_eq!(file_at(&st, ti), "zzz/y.py");
    }
}
