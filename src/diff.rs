//! Unified-diff parsing and change-block indexing (pure).

use std::collections::HashMap;

use crate::models::{LineInfo, Range};

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

    #[test]
    fn hunk_header_parsing() {
        // both start at line 1 (new-side START, not count)
        assert_eq!(parse_hunk_header("@@ -1,3 +1,4 @@ ctx"), Some((1, 1)));
        assert_eq!(parse_hunk_header("@@ -10 +11 @@"), Some((10, 11)));
    }
}
