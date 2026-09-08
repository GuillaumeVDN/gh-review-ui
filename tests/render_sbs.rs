//! Rendering test for the side-by-side review diff.

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use ghreview::diff::{compute_hunks, parse_diff};
use ghreview::models::{FileEntry, Focus, PendingComment, State, TreeRow};
use ghreview::ui;

fn review_state() -> State {
    let raw = "diff --git a/f.txt b/f.txt\nindex 1..2 100644\n--- a/f.txt\n+++ b/f.txt\n\
               @@ -1,4 +1,5 @@\n keep\n-old\n+new\n+extra\n tail\n";
    let mut st = State::default();
    st.files = vec![FileEntry { path: "f.txt".into(), viewed: false }];
    st.tree = vec![TreeRow::File { depth: 0, name: "f.txt".into(), index: 0 }];
    let (d, i) = parse_diff(raw);
    st.hunks_by_file = d.iter().map(|(p, l)| (p.clone(), compute_hunks(l))).collect();
    (st.diff_by_file, st.info_by_file) = (d, i);
    st.focus = Focus::Files;
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

/// The row holding `needle`, with its leading/trailing padding removed.
fn row_with<'a>(out: &'a str, needle: &str) -> &'a str {
    out.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no row with {needle}:\n{out}"))
}

#[test]
fn side_by_side_puts_the_old_line_across_from_the_new_one() {
    let mut st = review_state();
    st.side_by_side = true;
    let out = screen(&mut st, 160, 30);
    assert!(out.contains("old | new"), "the title marks the view: {out}");

    // The replacement reads across: `-old` left, `+new` right, on one row.
    let row = row_with(&out, "-old");
    assert!(row.contains("+new"), "{row}");
    assert!(row.find("-old").unwrap() < row.find("+new").unwrap(), "{row}");

    // The unpaired addition keeps the old side empty.
    let row = row_with(&out, "+extra");
    assert!(!row.contains("-"), "{row}");

    // Context sits in both columns.
    let row = row_with(&out, "keep");
    assert_eq!(row.matches("keep").count(), 2, "{row}");
}

#[test]
fn side_by_side_narrows_the_left_pane_and_the_inline_view_gives_it_back() {
    let mut st = review_state();
    let area = ratatui::layout::Rect::new(0, 0, 160, 30);
    st.side_by_side = true;
    let _ = screen(&mut st, 160, 30);
    let (rects, _, _) = ui::compute_layout(area, &st);
    assert_eq!(rects.prs.width, 32);

    st.side_by_side = false;
    let (rects, _, _) = ui::compute_layout(area, &st);
    assert_eq!(rects.prs.width, 53);
}

#[test]
fn side_by_side_keeps_the_inline_pending_comment() {
    let mut st = review_state();
    st.side_by_side = true;
    st.pending = vec![PendingComment {
        path: "f.txt".into(),
        body: "needs a test".into(),
        line: 2,
        side: "RIGHT".into(),
        comment_id: "local-1".into(),
        start_line: None,
        start_side: "RIGHT".into(),
    }];
    let out = screen(&mut st, 160, 30);
    let row = row_with(&out, "needs a test");
    assert!(row.contains("💬"), "inline under its line: {row}");
}

#[test]
fn side_by_side_renders_in_a_tiny_terminal() {
    let mut st = review_state();
    st.side_by_side = true;
    for (w, h) in [(60u16, 12u16), (40, 8), (24, 6), (10, 4)] {
        let _ = screen(&mut st, w, h); // must not panic
    }
}
