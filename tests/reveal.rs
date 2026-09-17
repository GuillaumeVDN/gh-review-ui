//! A `j`/`k` jump in the diff pane must land where the reader can see it.

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use ghreview::diff::{compute_hunks, parse_diff};
use ghreview::models::{FileEntry, Focus, State, TreeRow};
use ghreview::navigation as nav;
use ghreview::ui;

/// A two-hunk diff whose context lines are too long for one row, so every line
/// takes several rows of the pane.
fn wrapped_state() -> State {
    let long = "x".repeat(200);
    let mut raw = String::from(
        "diff --git a/f.txt b/f.txt\nindex 1..2 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,24 +1,24 @@\n",
    );
    raw.push_str("-first old\n+FIRST_NEW\n");
    for _ in 0..20 {
        raw.push_str(&format!(" {long}\n"));
    }
    raw.push_str("-last old\n+LAST_NEW\n tail\n");

    let mut st = State::default();
    st.files = vec![FileEntry { path: "f.txt".into(), viewed: false }];
    st.tree = vec![TreeRow::File { depth: 0, name: "f.txt".into(), index: 0 }];
    let (d, i) = parse_diff(&raw);
    st.hunks_by_file = d.iter().map(|(p, l)| (p.clone(), compute_hunks(l))).collect();
    (st.diff_by_file, st.info_by_file) = (d, i);
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
fn the_last_hunk_of_a_wrapped_diff_shows_on_screen() {
    for sbs in [false, true] {
        let mut st = wrapped_state();
        st.side_by_side = sbs;
        let out = screen(&mut st, 160, 30);
        assert!(out.contains("FIRST_NEW"), "the first hunk opens the diff (sbs={sbs}):\n{out}");

        nav::jump_stop(&mut st, 1);
        let out = screen(&mut st, 160, 30);
        assert!(out.contains("LAST_NEW"), "the last hunk is on screen (sbs={sbs}):\n{out}");

        nav::jump_stop(&mut st, -1);
        let out = screen(&mut st, 160, 30);
        assert!(out.contains("FIRST_NEW"), "the jump back is on screen (sbs={sbs}):\n{out}");
    }
}
