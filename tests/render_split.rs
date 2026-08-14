//! Rendering smoke test for the split (unstaged | staged) local diff.

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use ghreview::diff::{compute_hunks, parse_diff};
use ghreview::models::{EditEntry, EditKind, Focus, State, TreeRow};
use ghreview::ui;

fn split_state() -> State {
    let unstaged = "diff --git a/f.txt b/f.txt\nindex 1..2 100644\n--- a/f.txt\n+++ b/f.txt\n\
                    @@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
    let staged = "diff --git a/f.txt b/f.txt\nindex 3..4 100644\n--- a/f.txt\n+++ b/f.txt\n\
                  @@ -3,3 +3,3 @@\n c\n-d\n+D\n e\n";
    let combined = "diff --git a/f.txt b/f.txt\nindex 1..4 100644\n--- a/f.txt\n+++ b/f.txt\n\
                    @@ -1,5 +1,5 @@\n a\n-b\n+B\n c\n-d\n+D\n e\n";
    let mut st = State::default();
    st.edit_files = vec![EditEntry { path: "f.txt".into(), kind: EditKind::Modified }];
    st.edit_tree = vec![TreeRow::File { depth: 0, name: "f.txt".into(), index: 0 }];
    let parse = |raw: &str| {
        let (d, i) = parse_diff(raw);
        let h = d.iter().map(|(p, l)| (p.clone(), compute_hunks(l))).collect();
        (d, i, h)
    };
    (st.edit_diff_by_file, st.edit_info_by_file, st.edit_hunks_by_file) = parse(combined);
    (st.unstaged_diff_by_file, st.unstaged_info_by_file, st.unstaged_hunks_by_file) = parse(unstaged);
    (st.staged_diff_by_file, st.staged_info_by_file, st.staged_hunks_by_file) = parse(staged);
    st.local_diff_path = Some("f.txt".into());
    st.focus = Focus::Diff;
    st
}

fn screen(st: &mut State, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::render(f, st)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn split_shows_both_columns_and_narrows_the_left_pane() {
    let mut st = split_state();
    let out = screen(&mut st, 160, 30);
    assert!(out.contains("Unstaged (1)"), "{out}");
    assert!(out.contains("Staged (1)"), "{out}");
    assert!(out.contains("unstaged | staged"), "title marks the split: {out}");
    // Each column carries only its own change.
    assert!(out.contains("+B") && out.contains("+D"), "{out}");
    // The left pane column shrinks to make room (1/5 instead of 1/3 of 160).
    let (rects, _, _) = ui::compute_layout(ratatui::layout::Rect::new(0, 0, 160, 30), &st);
    assert_eq!(rects.prs.width, 32);
    st.staged_diff_by_file.remove("f.txt"); // nothing staged → single column again
    let (rects, _, _) = ui::compute_layout(ratatui::layout::Rect::new(0, 0, 160, 30), &st);
    assert_eq!(rects.prs.width, 53);
}

#[test]
fn split_renders_in_a_tiny_terminal() {
    let mut st = split_state();
    for (w, h) in [(60u16, 12u16), (40, 8), (24, 6)] {
        let _ = screen(&mut st, w, h); // must not panic
    }
}
