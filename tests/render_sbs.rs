//! Rendering test for the side-by-side review diff.

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use ghreview::diff::{compute_hunks, parse_diff};
use ghreview::models::{
    FileEntry, Focus, PendingComment, ReviewThread, State, ThreadComment, TreeRow,
};
use ghreview::ui;

fn review_state() -> State {
    let raw = "diff --git a/f.txt b/f.txt\nindex 1..2 100644\n--- a/f.txt\n+++ b/f.txt\n\
               @@ -8,4 +8,5 @@\n keep\n-old\n+new\n+extra\n tail\n";
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
fn side_by_side_numbers_the_old_side_left_and_the_new_side_right() {
    let mut st = review_state();
    st.side_by_side = true;
    let out = screen(&mut st, 160, 30);

    // `-old` is old line 9, and the `+new` that replaces it is new line 9.
    let row = row_with(&out, "-old");
    assert!(row.contains("9 -old"), "{row}");
    assert!(row.contains("9 +new"), "{row}");

    // The unpaired addition numbers the new side alone.
    let row = row_with(&out, "+extra");
    assert!(row.contains("10 +extra"), "{row}");

    // Context carries a number on each side, and the two sides drift apart
    // after the extra line: old 10, new 11.
    let row = row_with(&out, "tail");
    assert!(row.contains("10  tail"), "{row}");
    assert!(row.contains("11  tail"), "{row}");
}

#[test]
fn inline_numbers_both_sides_in_one_gutter() {
    let mut st = review_state();
    let out = screen(&mut st, 160, 30);
    assert!(!out.contains("old | new"), "the inline view: {out}");

    // Old then new, and a changed line only fills its own side.
    assert!(row_with(&out, " keep").contains("8   8  keep"), "{out}");
    assert!(row_with(&out, "-old").contains("9     -old"), "{out}");
    assert!(row_with(&out, "+new").contains("9 +new"), "{out}");
    assert!(row_with(&out, " tail").contains("10  11  tail"), "{out}");
}

#[test]
fn a_narrow_pane_drops_the_gutter_rather_than_the_code() {
    let mut st = review_state();
    st.side_by_side = true;
    let out = screen(&mut st, 58, 14);
    let row = row_with(&out, "-old");
    assert!(row.contains("+new"), "both columns still readable: {row}");
    assert!(!row.contains('8') && !row.contains('9'), "no room for the gutter: {row}");
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
        line: 9,
        side: "RIGHT".into(),
        comment_id: "local-1".into(),
        start_line: None,
        start_side: "RIGHT".into(),
    }];
    let out = screen(&mut st, 160, 30);
    let row = row_with(&out, "needs a test");
    assert!(row.contains("💬"), "inline under its line: {row}");
}

fn pending_on_9() -> PendingComment {
    PendingComment {
        path: "f.txt".into(),
        body: "needs a test".into(),
        line: 9,
        side: "RIGHT".into(),
        comment_id: "local-1".into(),
        start_line: None,
        start_side: "RIGHT".into(),
    }
}

fn thread_on(line: Option<i64>, bodies: &[&str]) -> ReviewThread {
    ReviewThread {
        id: "t1".into(),
        path: "f.txt".into(),
        line,
        start_line: None,
        side: "RIGHT".into(),
        start_side: "RIGHT".into(),
        outdated: line.is_none(),
        original_line: Some(42),
        comments: bodies
            .iter()
            .map(|b| ThreadComment {
                author: "bob".into(),
                body: (*b).into(),
                created_at: "2024-05-01T12:00:00Z".into(),
                url: "https://github.com/o/r/pull/1#discussion_r1".into(),
            })
            .collect(),
    }
}

#[test]
fn an_unresolved_thread_reads_under_its_line_in_both_views() {
    let mut st = review_state();
    st.threads = vec![thread_on(Some(9), &["please rework this"])];
    let out = screen(&mut st, 160, 30);
    assert!(row_with(&out, "@bob").contains("●"), "the header names the author: {out}");
    assert!(out.contains("please rework this"), "{out}");
    assert!(out.contains("1 thread"), "the title counts them: {out}");

    st.side_by_side = true;
    let out = screen(&mut st, 160, 30);
    assert!(out.contains("please rework this"), "{out}");
}

/// An outdated thread the diff cannot place reads at the top of the file, with
/// the line it was written on.
#[test]
fn a_thread_without_a_line_reads_at_the_top() {
    let mut st = review_state();
    st.threads = vec![thread_on(None, &["long gone"])];
    let out = screen(&mut st, 160, 30);
    let row = row_with(&out, "@bob");
    assert!(row.contains("(outdated)"), "{row}");
    assert!(row.contains("was line 42"), "{row}");
}

#[test]
fn the_focused_comment_stop_wears_the_side_bar() {
    let mut st = review_state();
    st.focus = Focus::Diff;
    st.pending = vec![pending_on_9()];
    // The block first, then the comment hanging under its last line.
    let out = screen(&mut st, 160, 30);
    assert!(row_with(&out, "needs a test").contains(" ▏💬"), "unfocused: {out}");

    st.diff_stop_idx = 1;
    let out = screen(&mut st, 160, 30);
    assert!(row_with(&out, "needs a test").contains("▌▏💬"), "focused: {out}");
}

/// A long thread gives way to a count, and the focused one shows whole.
#[test]
fn a_long_thread_folds_until_it_is_the_stop() {
    let body: String = (0..20).map(|i| format!("line {i}\n")).collect();
    let mut st = review_state();
    st.focus = Focus::Diff;
    st.threads = vec![thread_on(Some(9), &[&body])];
    let out = screen(&mut st, 160, 40);
    assert!(out.contains("more lines"), "folded: {out}");
    assert!(!out.contains("line 19"), "folded: {out}");

    st.diff_stop_idx = 1;
    let out = screen(&mut st, 160, 40);
    assert!(!out.contains("more lines"), "the stop shows whole: {out}");
    assert!(out.contains("line 19"), "{out}");
}

#[test]
fn side_by_side_renders_in_a_tiny_terminal() {
    let mut st = review_state();
    st.side_by_side = true;
    for (w, h) in [(60u16, 12u16), (40, 8), (24, 6), (10, 4)] {
        let _ = screen(&mut st, w, h); // must not panic
    }
}
