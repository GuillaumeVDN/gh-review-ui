//! File-tree building and folding for the Files pane (pure).

use std::collections::{HashMap, HashSet};

use crate::models::{FileEntry, State, TreeRow};

enum Node {
    Dir(HashMap<String, Node>),
    File(usize),
}

/// Build the display rows (dirs before files, case-insensitive) for `files`.
pub fn build_tree(files: &[FileEntry], collapsed: &HashSet<String>) -> Vec<TreeRow> {
    let mut root = Node::Dir(HashMap::new());
    for (i, f) in files.iter().enumerate() {
        let parts: Vec<&str> = f.path.split('/').collect();
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

/// Tree index of the first unviewed file, if any.
pub fn first_unviewed_index(st: &State) -> Option<usize> {
    st.tree.iter().position(|row| match row {
        TreeRow::File { index, .. } => !st.files[*index].viewed,
        _ => false,
    })
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
}
