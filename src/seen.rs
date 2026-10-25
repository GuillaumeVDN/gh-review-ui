//! The changed lines already looked at, kept per file across reloads.
//!
//! GitHub holds one viewed bit per file and drops it when a push touches the
//! file. A key per changed line remembers what the reviewer saw, so a new
//! commit shows only its own lines at full strength.

use std::collections::{HashMap, HashSet};

/// The key of each diff line, `None` for the lines that change nothing.
///
/// A key covers the line and its two neighbours. The line number cannot take
/// part, because an earlier hunk moves it. The neighbours tell apart two
/// identical lines, such as two closing braces, in different places.
pub fn line_keys(lines: &[String]) -> Vec<Option<u64>> {
    let neighbour = |i: Option<usize>| -> &str {
        match i.and_then(|i| lines.get(i)) {
            Some(l) if l.starts_with("@@") => "@@",
            Some(l) => l,
            None => "",
        }
    };
    (0..lines.len())
        .map(|i| {
            let l = &lines[i];
            if !crate::diff::is_add(l) && !crate::diff::is_del(l) {
                return None;
            }
            Some(fnv(&[neighbour(i.checked_sub(1)), l, neighbour(Some(i + 1))]))
        })
        .collect()
}

/// FNV-1a: the keys go to disk, so the hash must not change between builds.
fn fnv(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for part in parts {
        for b in part.bytes().chain([0]) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// The files with some changed lines seen and some not.
pub fn partial(
    keys: &HashMap<String, Vec<Option<u64>>>,
    seen: &HashMap<String, HashSet<u64>>,
) -> HashSet<String> {
    keys.iter()
        .filter(|(path, ks)| {
            let Some(s) = seen.get(*path) else { return false };
            let ks: Vec<u64> = ks.iter().flatten().copied().collect();
            ks.iter().any(|k| s.contains(k)) && ks.iter().any(|k| !s.contains(k))
        })
        .map(|(path, _)| path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_changed_lines_have_a_key() {
        let keys = line_keys(&lines(&["diff --git a/x b/x", "--- a/x", "+++ b/x", "@@ -1 +1 @@", " a", "-b", "+c"]));
        let has: Vec<bool> = keys.iter().map(Option::is_some).collect();
        assert_eq!(has, [false, false, false, false, false, true, true]);
    }

    /// A commit that adds a hunk above moves every line number below it. The
    /// lines below were seen, and they stay seen.
    #[test]
    fn a_hunk_above_keeps_the_lines_below_seen() {
        let before = lines(&["@@ -10,3 +10,3 @@", " a", "-b", "+c", " d"]);
        let after = lines(&["@@ -1,2 +1,3 @@", " x", "+new", " y", "@@ -10,3 +11,3 @@", " a", "-b", "+c", " d"]);
        let seen: HashSet<u64> = line_keys(&before).into_iter().flatten().collect();
        let keys = line_keys(&after);
        let is_seen: Vec<bool> = keys.iter().map(|k| k.is_some_and(|k| seen.contains(&k))).collect();
        assert_eq!(is_seen, [false, false, false, false, false, false, true, true, false]);
    }

    #[test]
    fn the_same_line_in_another_place_is_not_seen() {
        let before = lines(&["@@ -1,2 +1,3 @@", " fn a() {", "+}", " "]);
        let after = lines(&["@@ -1,2 +1,3 @@", " fn a() {", "+}", " ", "@@ -9,2 +10,3 @@", " fn b() {", "+}", " "]);
        let seen: HashSet<u64> = line_keys(&before).into_iter().flatten().collect();
        let keys = line_keys(&after);
        assert!(seen.contains(&keys[2].unwrap()));
        assert!(!seen.contains(&keys[6].unwrap()));
    }

    #[test]
    fn a_file_is_partial_only_with_both_seen_and_new_lines() {
        let diff = lines(&["@@ -1,3 +1,3 @@", " a", "-b", "+c", " d"]);
        let ks = line_keys(&diff);
        let all: HashSet<u64> = ks.iter().flatten().copied().collect();
        let one: HashSet<u64> = ks.iter().flatten().take(1).copied().collect();
        let keys: HashMap<String, Vec<Option<u64>>> =
            [("all.rs".into(), ks.clone()), ("one.rs".into(), ks.clone()), ("none.rs".into(), ks)].into_iter().collect();
        let seen: HashMap<String, HashSet<u64>> = [("all.rs".into(), all), ("one.rs".into(), one)].into_iter().collect();
        assert_eq!(partial(&keys, &seen), ["one.rs".to_string()].into_iter().collect());
    }
}
