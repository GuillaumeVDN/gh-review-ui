//! Unified-diff parsing and change-block indexing (pure).

use std::collections::{HashMap, HashSet};

use crate::models::{EditEntry, EditKind, LineInfo, Range};

/// Parse `@@ -old[,n] +new[,n] @@` → (old_start, new_start).
pub fn parse_hunk_header(line: &str) -> Option<(i64, i64)> {
    let rest = line.strip_prefix("@@ -")?;
    let mut parts = rest.splitn(2, " +");
    let old_part = parts.next()?;
    let new_part = parts.next()?;
    let old = old_part.split(',').next()?.trim();
    let new = new_part.split([',', ' ']).next()?.trim();
    Some((old.parse().ok()?, new.parse().ok()?))
}

fn is_add(line: &str) -> bool {
    line.starts_with('+') && !line.starts_with("+++")
}
fn is_del(line: &str) -> bool {
    line.starts_with('-') && !line.starts_with("---")
}

/// Split a `git diff` into per-file line lists and per-file line info.
///
/// `per_info[path][i]` is `(old_line_no?, new_line_no?)` for diff row `i`.
pub fn parse_diff(raw: &str) -> (HashMap<String, Vec<String>>, HashMap<String, Vec<LineInfo>>) {
    let mut per_file: HashMap<String, Vec<String>> = HashMap::new();
    let mut per_info: HashMap<String, Vec<LineInfo>> = HashMap::new();

    let mut current: Option<String> = None;
    let mut buf: Vec<String> = Vec::new();
    let mut info: Vec<LineInfo> = Vec::new();
    let mut old_no = 0i64;
    let mut new_no = 0i64;
    let mut in_hunk = false;

    macro_rules! flush {
        () => {
            if let Some(path) = current.take() {
                per_file.insert(path.clone(), std::mem::take(&mut buf));
                per_info.insert(path, std::mem::take(&mut info));
            }
        };
    }

    for line in raw.split('\n') {
        if line.starts_with("diff --git ") {
            flush!();
            buf = vec![line.to_string()];
            info = vec![(None, None)];
            current = line.split(" b/").nth(1).map(str::to_string);
            in_hunk = false;
            old_no = 0;
            new_no = 0;
            continue;
        }
        if current.is_none() {
            continue;
        }
        buf.push(line.to_string());
        if line.starts_with("@@") {
            if let Some((o, n)) = parse_hunk_header(line) {
                old_no = o;
                new_no = n;
                in_hunk = true;
            }
            info.push((None, None));
            continue;
        }
        if !in_hunk {
            info.push((None, None));
            continue;
        }
        if is_add(line) {
            info.push((None, Some(new_no)));
            new_no += 1;
        } else if is_del(line) {
            info.push((Some(old_no), None));
            old_no += 1;
        } else if line.starts_with('\\') {
            info.push((None, None)); // "\ No newline at end of file"
        } else {
            info.push((Some(old_no), Some(new_no)));
            old_no += 1;
            new_no += 1;
        }
    }
    flush!();
    (per_file, per_info)
}

/// Classify one file's diff block as added / deleted / modified from its
/// `new file mode` / `deleted file mode` markers (else a content change).
pub fn edit_kind(diff_lines: &[String]) -> EditKind {
    for l in diff_lines {
        if l.starts_with("new file mode") {
            return EditKind::Added;
        }
        if l.starts_with("deleted file mode") {
            return EditKind::Deleted;
        }
    }
    EditKind::Modified
}

/// Parse a `git diff` of the worktree into the list of changed files (sorted,
/// with their change kind). This is exactly the set that will be committed, so
/// it must never pick up anything outside the diff.
pub fn classify_edits(raw: &str) -> Vec<EditEntry> {
    let (per_file, _) = parse_diff(raw);
    let mut out: Vec<EditEntry> = per_file
        .iter()
        .map(|(path, lines)| EditEntry { path: path.clone(), kind: edit_kind(lines) })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn strip_diff_marker(l: &str) -> &str {
    if l.starts_with(['+', '-', ' ']) {
        &l[1..]
    } else {
        l
    }
}

/// How to overlay a *local* diff (PR head → worktree) onto the PR review diff.
///
/// The local diff's old side is the PR head, which is exactly the review diff's
/// new side — so we key everything by head line number:
/// - `deleted_heads`: head lines removed/replaced locally (drawn struck-through);
/// - `adds_after`: new local content to insert right after a given head line;
/// - `adds_top`: local content added before the first head line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalOverlay {
    pub deleted_heads: HashSet<i64>,
    pub adds_after: HashMap<i64, Vec<String>>,
    pub adds_top: Vec<String>,
}

pub fn local_overlay(lines: &[String], info: &[LineInfo]) -> LocalOverlay {
    let mut ov = LocalOverlay::default();
    let mut last_head = 0i64;
    for (i, &(old, new)) in info.iter().enumerate() {
        match (old, new) {
            (Some(h), None) => {
                // Head line removed (or the old half of a modification).
                ov.deleted_heads.insert(h);
                last_head = h;
            }
            (None, Some(_)) => {
                // New local content — anchor it after the last head line seen.
                let content = strip_diff_marker(lines.get(i).map(String::as_str).unwrap_or("")).to_string();
                if last_head == 0 {
                    ov.adds_top.push(content);
                } else {
                    ov.adds_after.entry(last_head).or_default().push(content);
                }
            }
            (Some(h), Some(_)) => last_head = h, // unchanged head line
            (None, None) => {}                   // header / hunk / marker line
        }
    }
    ov
}

/// Contiguous runs of changed (`+`/`-`) lines — the app's navigable "hunks".
///
/// Context lines, `@@` headers and file headers break a run, so extended
/// context around edits never merges separate changes into one giant block.
pub fn compute_hunks(diff_lines: &[String]) -> Vec<Range> {
    let mut blocks = Vec::new();
    let mut start: Option<usize> = None;
    for (i, ln) in diff_lines.iter().enumerate() {
        let changed = is_add(ln) || is_del(ln);
        if changed {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            blocks.push((s, i));
        }
    }
    if let Some(s) = start {
        blocks.push((s, diff_lines.len()));
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        [
            "diff --git a/foo.py b/foo.py",
            "index 111..222 100644",
            "--- a/foo.py",
            "+++ b/foo.py",
            "@@ -1,3 +1,4 @@",
            " ctx",
            "-old line",
            "+new line",
            "+added line",
            "@@ -10,2 +11,2 @@",
            " keep",
            "-gone",
            "+fresh",
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn splits_files_and_maps_lines() {
        let (files, info) = parse_diff(&sample());
        assert!(files.contains_key("foo.py"));
        let fl = &files["foo.py"];
        let fi = &info["foo.py"];
        let i = fl.iter().position(|l| l == "+new line").unwrap();
        assert_eq!(fi[i], (None, Some(2)));
        let j = fl.iter().position(|l| l == "-old line").unwrap();
        assert_eq!(fi[j], (Some(2), None));
        let k = fl.iter().position(|l| l == " ctx").unwrap();
        assert_eq!(fi[k], (Some(1), Some(1)));
    }

    #[test]
    fn hunks_are_change_blocks() {
        let (files, _) = parse_diff(&sample());
        let fl = &files["foo.py"];
        let hunks = compute_hunks(fl);
        assert_eq!(hunks.len(), 2);
        for &(s, e) in &hunks {
            for ln in &fl[s..e] {
                assert!(
                    (ln.starts_with('+') || ln.starts_with('-'))
                        && !ln.starts_with("+++")
                        && !ln.starts_with("---")
                );
            }
        }
        assert_eq!(fl[hunks[0].0], "-old line");
        assert_eq!(fl[hunks[1].0], "-gone");
    }

    #[test]
    fn context_splits_adjacent_changes() {
        let raw = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,4 @@\n\
-test\n+test2\n context\n-test3\n+test4\n";
        let (files, _) = parse_diff(raw);
        assert_eq!(compute_hunks(&files["f"]).len(), 2);
    }

    #[test]
    fn empty() {
        let (f, i) = parse_diff("");
        assert!(f.is_empty() && i.is_empty());
        assert!(compute_hunks(&[]).is_empty());
    }

    fn added_file() -> &'static str {
        "diff --git a/new.txt b/new.txt\n\
         new file mode 100644\n\
         index 0000000..e69de29\n\
         --- /dev/null\n\
         +++ b/new.txt\n\
         @@ -0,0 +1,2 @@\n\
         +hello\n\
         +world\n"
    }
    fn deleted_file() -> &'static str {
        "diff --git a/gone.txt b/gone.txt\n\
         deleted file mode 100644\n\
         index e69de29..0000000\n\
         --- a/gone.txt\n\
         +++ /dev/null\n\
         @@ -1 +0,0 @@\n\
         -bye\n"
    }
    fn modified_file() -> &'static str {
        "diff --git a/mod.txt b/mod.txt\n\
         index 1111111..2222222 100644\n\
         --- a/mod.txt\n\
         +++ b/mod.txt\n\
         @@ -1 +1 @@\n\
         -old\n\
         +new\n"
    }

    #[test]
    fn edit_kind_from_markers() {
        let (f, _) = parse_diff(added_file());
        assert_eq!(edit_kind(&f["new.txt"]), EditKind::Added);
        let (f, _) = parse_diff(deleted_file());
        assert_eq!(edit_kind(&f["gone.txt"]), EditKind::Deleted);
        let (f, _) = parse_diff(modified_file());
        assert_eq!(edit_kind(&f["mod.txt"]), EditKind::Modified);
    }

    #[test]
    fn classify_edits_lists_only_diffed_files() {
        let raw = format!("{}{}{}", added_file(), deleted_file(), modified_file());
        let edits = classify_edits(&raw);
        assert_eq!(
            edits.iter().map(|e| (e.path.as_str(), e.kind)).collect::<Vec<_>>(),
            vec![
                ("gone.txt", EditKind::Deleted),
                ("mod.txt", EditKind::Modified),
                ("new.txt", EditKind::Added),
            ]
        );
    }

    #[test]
    fn classify_edits_empty_is_nothing() {
        // No diff → nothing to commit.
        assert!(classify_edits("").is_empty());
        assert!(classify_edits("\n").is_empty());
    }

    #[test]
    fn classify_edits_rename_keeps_only_new_path() {
        // A rename lists the new path (what will be committed), not the old one.
        let raw = "diff --git a/old.txt b/new.txt\n\
                   similarity index 100%\n\
                   rename from old.txt\n\
                   rename to new.txt\n";
        let edits = classify_edits(raw);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].path, "new.txt");
        assert_eq!(edits[0].kind, EditKind::Modified);
    }

    #[test]
    fn local_overlay_maps_changes() {
        // Local diff: modify head line 47, delete head line 49.
        let raw = "diff --git a/f b/f\n--- a/f\n+++ b/f\n\
                   @@ -46,5 +46,4 @@\n ctx46\n-old47\n+new47\n ctx48\n-gone49\n";
        let (files, info) = parse_diff(raw);
        let ov = local_overlay(&files["f"], &info["f"]);
        assert!(ov.deleted_heads.contains(&47) && ov.deleted_heads.contains(&49));
        assert_eq!(ov.deleted_heads.len(), 2);
        assert_eq!(ov.adds_after.get(&47), Some(&vec!["new47".to_string()]));
        assert!(ov.adds_top.is_empty());
    }

    #[test]
    fn local_overlay_top_addition() {
        // A pure addition before any head line lands in `adds_top`.
        let raw = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -0,0 +1,2 @@\n+first\n+second\n";
        let (files, info) = parse_diff(raw);
        let ov = local_overlay(&files["f"], &info["f"]);
        assert_eq!(ov.adds_top, vec!["first".to_string(), "second".to_string()]);
        assert!(ov.deleted_heads.is_empty() && ov.adds_after.is_empty());
    }

    #[test]
    fn classify_edits_binary_is_modified() {
        let raw = "diff --git a/img.png b/img.png\n\
                   index 1111111..2222222 100644\n\
                   Binary files a/img.png and b/img.png differ\n";
        let edits = classify_edits(raw);
        assert_eq!(edits.iter().map(|e| (e.path.as_str(), e.kind)).collect::<Vec<_>>(),
                   vec![("img.png", EditKind::Modified)]);
    }

    #[test]
    fn hunk_header_parsing() {
        // both start at line 1 (new-side START, not count)
        assert_eq!(parse_hunk_header("@@ -1,3 +1,4 @@ ctx"), Some((1, 1)));
        assert_eq!(parse_hunk_header("@@ -10 +11 @@"), Some((10, 11)));
    }
}
