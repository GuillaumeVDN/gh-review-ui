//! Ratatui rendering of the panes + modal overlays.

use std::collections::HashSet;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::markdown::{format_pr_details, wrap_styled};
use crate::models::{
    CommitKind, Focus, Overlay, PendingComment, StageState, State, TreeRow, SUBMIT_CHOICES,
};
use crate::navigation::{
    current_hunk_range, diff_path, hunk_for_comment, is_local_diff, is_split, source_maps,
    stage_state,
};
use crate::textbuffer;
use crate::theme;

/// Rectangles of the five left panes + the right pane (for mouse hit-testing).
pub struct PaneRects {
    pub prs: Rect,
    pub commits: Rect,
    pub files: Rect,
    pub edits: Rect,
    pub pending: Rect,
    pub right: Rect,
    pub body: Rect,
}

/// Whether the [0] pane is showing a split (unstaged | staged) local diff, which
/// needs the extra width taken from the left column.
pub fn shows_split_diff(st: &State) -> bool {
    st.local_diff_path.as_deref().map_or(false, |p| is_split(st, p))
}

pub fn compute_layout(area: Rect, st: &State) -> (PaneRects, Rect, Rect) {
    compute_layout_at(area, st.focus, st.local_diff_path.is_some(), shows_split_diff(st))
}

pub fn compute_layout_at(area: Rect, focus: Focus, local_diff: bool, split: bool) -> (PaneRects, Rect, Rect) {
    let root = Layout::vertical([Constraint::Min(0), Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    let (body, status, help) = (root[0], root[1], root[2]);
    // Two diff columns need the room: shrink the left panes while one is shown.
    let left_w = if split {
        (area.width / 5).max(30)
    } else {
        (area.width / 3).max(38)
    }
    .min(area.width.saturating_sub(20));
    let cols = Layout::horizontal([Constraint::Length(left_w), Constraint::Min(0)]).split(body);
    let (left, right) = (cols[0], cols[1]);
    // The focused left pane (or Files while viewing the diff) expands to fill the
    // column; the others shrink to a compact height. Left-pane order: PRs,
    // Commits, Files, Edits, Pending. Viewing a *local* diff keeps [4] expanded.
    let expanded = match focus {
        Focus::Prs => 0,
        Focus::Commits => 1,
        Focus::Files => 2,
        Focus::Diff => if local_diff { 3 } else { 2 },
        Focus::Edits => 3,
        Focus::Pending => 4,
    };
    let small = [6u16, 5, 5, 4, 4];
    let constraints: Vec<Constraint> = (0..5)
        .map(|i| if i == expanded { Constraint::Min(3) } else { Constraint::Length(small[i]) })
        .collect();
    let rows = Layout::vertical(constraints).split(left);
    (
        PaneRects {
            prs: rows[0],
            commits: rows[1],
            files: rows[2],
            edits: rows[3],
            pending: rows[4],
            right,
            body,
        },
        status,
        help,
    )
}

fn clamp_view(idx: usize, offset: usize, vh: usize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    if idx < offset {
        return idx;
    }
    if vh > 0 && idx >= offset + vh {
        return idx + 1 - vh;
    }
    let max_off = total.saturating_sub(vh);
    offset.min(max_off)
}

/// Minimal scroll so `[lo, hi)` is visible in `vh` rows.
pub fn reveal_scroll(scroll: usize, lo: usize, hi: usize, vh: usize) -> usize {
    if vh == 0 {
        return scroll;
    }
    if lo < scroll {
        return lo;
    }
    if hi > scroll + vh {
        return if hi - lo > vh { lo } else { hi - vh };
    }
    scroll
}

/// True while a PR checkout is in flight (worktree fetch + active load), so the
/// panes read "Loading…" instead of stale/empty content.
fn is_loading(st: &State) -> bool {
    st.busy.contains("worktree") || st.busy.contains("active")
}

fn block(title: &str, focused: bool, busy: bool) -> Block<'static> {
    let bs = if focused { theme::border_focused() } else { theme::border_dim() };
    let ts = if focused { theme::border_focused() } else { theme::title() };
    let label = format!(" {title}{} ", if busy { " ⏳" } else { "" });
    Block::default().borders(Borders::ALL).border_style(bs).title(Span::styled(label, ts))
}

fn pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w {
        s.chars().take(w).collect()
    } else {
        format!("{s}{}", " ".repeat(w - n))
    }
}

/// Hard-wrap a string into rows of at most `width` chars (column-based, so long
/// code lines are split rather than truncated). Empty input yields one empty row.
fn wrap_hard(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars.chunks(width).map(|c| c.iter().collect()).collect()
}

/// Shared tree/list renderer: given one `(text, base_style)` per row, handle
/// the scroll window and the selection pastille. Used by the Files and
/// Pending-edits panes.
fn render_rows(f: &mut Frame, inner: Rect, rows: &[(String, Style)], sel_idx: usize, offset: &mut usize, focused: bool) {
    let (vh, iw) = (inner.height as usize, inner.width as usize);
    *offset = clamp_view(sel_idx, *offset, vh, rows.len());
    let mut lines = Vec::new();
    for (i, (text, base)) in rows.iter().enumerate().skip(*offset).take(vh) {
        let selected = i == sel_idx && focused;
        lines.push(list_row(selected, text, *base, iw));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Word-wrap `body` to `width`, prefixing the first line with `prefix` and each
/// continuation line with a hanging indent the width of `prefix` (so wrapped
/// text lines up under the title). Over-long words are hard-split.
fn wrap_indented(prefix: &str, body: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let plen = prefix.chars().count();
    let avail = width.saturating_sub(plen).max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let cur_len = |s: &str| s.chars().count();
    for word in body.split(' ') {
        if cur_len(word) > avail {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            let chars: Vec<char> = word.chars().collect();
            for chunk in chars.chunks(avail) {
                lines.push(chunk.iter().collect());
            }
            cur = lines.pop().unwrap_or_default(); // keep the tail to append onto
            continue;
        }
        let would = if cur.is_empty() { cur_len(word) } else { cur_len(&cur) + 1 + cur_len(word) };
        if would > avail && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    let indent = " ".repeat(plen);
    lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| if i == 0 { format!("{prefix}{l}") } else { format!("{indent}{l}") })
        .collect()
}

/// Render a list whose rows may span several display lines. `rows` is the full
/// flattened set of display lines; `sel_line` is the display-line index to keep
/// visible. Each row is `(marker, marker_style, text, text_style)`.
fn render_line_list(
    f: &mut Frame,
    inner: Rect,
    rows: &[(&'static str, Style, String, Style)],
    sel_line: usize,
    offset: &mut usize,
) {
    let (vh, iw) = (inner.height as usize, inner.width as usize);
    *offset = clamp_view(sel_line, *offset, vh, rows.len());
    let mut out = Vec::new();
    for (marker, mstyle, text, tstyle) in rows.iter().skip(*offset).take(vh) {
        out.push(Line::from(vec![
            Span::styled(*marker, *mstyle),
            Span::styled(pad(text, iw.saturating_sub(1)), *tstyle),
        ]));
    }
    f.render_widget(Paragraph::new(out), inner);
}

/// A list row with a colored left-bar "pastille" when selected (no full-row
/// highlight — lazygit-style), otherwise the item's base style.
fn list_row(selected: bool, text: &str, base: Style, iw: usize) -> Line<'static> {
    let mark = if selected { "▌" } else { " " };
    let style = if selected { base.add_modifier(Modifier::BOLD) } else { base };
    Line::from(vec![
        Span::styled(mark.to_string(), theme::sel_marker()),
        Span::styled(pad(text, iw.saturating_sub(1)), style),
    ])
}

/// Section rows for the PRs pane: header labels + PR indices.
pub fn pr_rows(st: &State) -> Vec<(bool, String, usize)> {
    // (is_header, text, pr_index)
    use crate::models::Category;
    let mut rows = Vec::new();
    let mut last: Option<Category> = None;
    for (i, pr) in st.prs.iter().enumerate() {
        if last != Some(pr.category) {
            let (label, cat) = match pr.category {
                Category::CheckedOut => ("Checked out PR", Category::CheckedOut),
                Category::Mine => ("My PRs", Category::Mine),
                Category::Review => ("Requested review", Category::Review),
            };
            let n = st.prs.iter().filter(|p| p.category == cat).count();
            rows.push((true, format!("{label} ({n})"), 0));
            last = Some(pr.category);
        }
        rows.push((false, String::new(), i));
    }
    rows
}

pub fn render(f: &mut Frame, st: &mut State) {
    let (rects, status_area, help_area) = compute_layout(f.area(), st);
    render_prs(f, st, rects.prs);
    render_commits(f, st, rects.commits);
    render_files(f, st, rects.files);
    render_edits(f, st, rects.edits);
    render_pending(f, st, rects.pending);
    match st.focus {
        Focus::Prs => render_pr_details(f, st, rects.right),
        Focus::Commits => render_commit_detail(f, st, rects.right),
        Focus::Pending => render_pending_detail(f, st, rects.right),
        Focus::Edits => render_edit_diff(f, st, rects.right),
        _ => render_diff(f, st, rects.right),
    }

    let status = if st.status.is_empty() { "Ready." } else { &st.status };
    f.render_widget(Paragraph::new(status.to_string()).style(theme::status()), status_area);
    f.render_widget(Paragraph::new(shortcuts_for(st)).style(theme::keys()), help_area);

    render_overlay(f, st);
}

fn shortcuts_for(st: &State) -> String {
    let common = "r: refresh · q: quit";
    match st.focus {
        Focus::Prs => format!("Enter: open (worktree) · C: checkout local · o: open web · {common}"),
        Focus::Commits => format!("Space: toggle · a: all/none · Enter: apply range · {common}"),
        Focus::Pending => format!("j/k · Alt+j/k/z: next file · Enter: submit · e: edit · d: delete · {common}"),
        Focus::Files => format!("Enter: open/collapse · Space: viewed · e: editor · z/Z: fold/unfold · gg/G · {common}"),
        Focus::Edits => format!("Enter: hunks/fold · Space: stage · z/Z: fold/unfold · c: commit+push · A: amend · w: no hooks · P: push · e: editor · d: revert · {common}"),
        Focus::Diff if st.local_diff_path.is_some() => {
            format!("j/k: block · Space: stage hunk · d: revert hunk · h/l: column · c: comment · e: editor · Esc: back · {common}")
        }
        Focus::Diff => format!("j/k: block · c: comment/edit · a: ask Claude · e: editor · PgUp/Dn: scroll · Esc: back · {common}"),
    }
}

fn render_prs(f: &mut Frame, st: &mut State, area: Rect) {
    let title = format!("[1] PRs [{}/{}]", st.repo_owner, st.repo_name);
    let focused = st.focus == Focus::Prs;
    let b = block(&title, focused, st.busy.contains("prs"));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let iw = inner.width as usize;
    let tw = iw.saturating_sub(1);

    let mut rows: Vec<(&'static str, Style, String, Style)> = Vec::new();
    let mut sel_line = 0;
    for (is_hdr, text, idx) in pr_rows(st) {
        if is_hdr {
            rows.push((" ", theme::section_header(), text, theme::section_header()));
            continue;
        }
        let pr = &st.prs[idx];
        let active = st.active_pr.as_ref().map_or(false, |a| a.number == pr.number);
        let selected = idx == st.pr_idx && focused;
        let prefix = format!("{}#{} ", if active { "● " } else { "  " }, pr.number);
        let mut base = if active { theme::active_pr() } else { Style::default() };
        if selected {
            base = base.add_modifier(Modifier::BOLD);
        }
        for (li, line) in wrap_indented(&prefix, &pr.title, tw).into_iter().enumerate() {
            let first = li == 0;
            if selected && first {
                sel_line = rows.len();
            }
            let marker = if selected && first { "▌" } else { " " };
            rows.push((marker, theme::sel_marker(), line, base));
        }
    }
    if !focused {
        sel_line = st.pr_offset;
    }
    render_line_list(f, inner, &rows, sel_line, &mut st.pr_offset);
}

fn render_commits(f: &mut Frame, st: &mut State, area: Rect) {
    let title = if st.commits.is_empty() {
        "[2] Commits".to_string()
    } else {
        format!("[2] Commits ({}/{})", st.commit_selected.len(), st.commits.len())
    };
    let busy = st.busy.contains("active") || st.busy.contains("commitdiff");
    let focused = st.focus == Focus::Commits;
    let b = block(&title, focused, busy);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let tw = (inner.width as usize).saturating_sub(1);
    if st.commits.is_empty() {
        let msg = if is_loading(st) { "Loading…" } else { "No commits" };
        f.render_widget(Paragraph::new(Line::styled(msg, theme::dim())), inner);
        return;
    }
    let mut rows: Vec<(&'static str, Style, String, Style)> = Vec::new();
    let mut sel_line = 0;
    for (i, c) in st.commits.iter().enumerate() {
        let checked = st.commit_selected.contains(&c.oid);
        let selected = i == st.commit_idx && focused;
        let prefix = format!("[{}] {} ", if checked { "x" } else { " " }, c.short());
        let mut base = if checked { Style::default() } else { theme::dim() };
        if selected {
            base = base.add_modifier(Modifier::BOLD);
        }
        for (li, line) in wrap_indented(&prefix, &c.headline, tw).into_iter().enumerate() {
            let first = li == 0;
            if selected && first {
                sel_line = rows.len();
            }
            let marker = if selected && first { "▌" } else { " " };
            rows.push((marker, theme::sel_marker(), line, base));
        }
    }
    if !focused {
        sel_line = st.commit_offset;
    }
    render_line_list(f, inner, &rows, sel_line, &mut st.commit_offset);
}

fn render_files(f: &mut Frame, st: &mut State, area: Rect) {
    let mut title = "[3] Files".to_string();
    if let Some(pr) = &st.active_pr {
        let n = st.files.iter().filter(|f| f.viewed).count();
        title = format!("[3] Files #{}  {}/{} viewed", pr.number, n, st.files.len());
    }
    let busy = st.busy.contains("active") || st.busy.contains("viewed");
    let b = block(&title, st.focus == Focus::Files, busy);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let focused = st.focus == Focus::Files;
    if st.tree.is_empty() {
        let msg = if is_loading(st) { "Loading…" } else { "No files" };
        f.render_widget(Paragraph::new(Line::styled(msg, theme::dim())), inner);
        return;
    }
    let rows: Vec<(String, Style)> = st
        .tree
        .iter()
        .map(|row| match row {
            TreeRow::Dir { depth, name, collapsed, .. } => (
                format!("{}{} {}/", "  ".repeat(*depth), if *collapsed { "▶" } else { "▼" }, name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            TreeRow::File { depth, name, index } => {
                let path = &st.files[*index].path;
                let viewed = st.files[*index].viewed;
                let edit = st.edit_kind_by_path.get(path).copied();
                // Locally-edited files are colored (like [4]) to stand out from
                // already-pushed changes; the change kind is shown as a sigil.
                let base = match edit {
                    Some(k) => theme::edit_kind_style(k),
                    None if viewed => theme::dim(),
                    None => Style::default(),
                };
                let sigil = edit.map(|k| format!(" {}", k.sigil())).unwrap_or_default();
                (
                    format!("{}[{}] {}{sigil}", "  ".repeat(*depth), if viewed { "✔" } else { " " }, name),
                    base,
                )
            }
        })
        .collect();
    render_rows(f, inner, &rows, st.file_idx, &mut st.file_offset, focused);
}

fn render_edits(f: &mut Frame, st: &mut State, area: Rect) {
    let staged = st
        .edit_files
        .iter()
        .filter(|e| stage_state(st, &e.path) != StageState::Unstaged)
        .count();
    let title = if st.edit_files.is_empty() {
        "[4] Pending edits".to_string()
    } else if staged > 0 {
        format!("[4] Pending edits ({}·{staged}✔)", st.edit_files.len())
    } else {
        format!("[4] Pending edits ({})", st.edit_files.len())
    };
    let busy = st.busy.contains("edits") || st.busy.contains("editcommit");
    let b = block(&title, st.focus == Focus::Edits, busy);
    let inner = b.inner(area);
    f.render_widget(b, area);
    if st.edit_files.is_empty() {
        let msg = if is_loading(st) || st.busy.contains("edits") { "Loading…" } else { "No local changes" };
        f.render_widget(Paragraph::new(Line::styled(msg, theme::dim())), inner);
        return;
    }
    let focused = st.focus == Focus::Edits;
    let rows: Vec<(String, Style)> = st
        .edit_tree
        .iter()
        .map(|row| match row {
            TreeRow::Dir { depth, name, collapsed, .. } => (
                format!("{}{} {}/", "  ".repeat(*depth), if *collapsed { "▶" } else { "▼" }, name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            TreeRow::File { depth, name, index } => {
                let entry = &st.edit_files[*index];
                let stage = stage_state(st, &entry.path);
                // Fully staged reads as "done" (like a viewed PR file); a partial
                // stage keeps the change color so it still asks for attention.
                let style = if stage == StageState::Staged {
                    theme::dim()
                } else {
                    theme::edit_kind_style(entry.kind)
                };
                (
                    format!(
                        "{}[{}] {} {}",
                        "  ".repeat(*depth),
                        stage.mark(),
                        entry.kind.sigil(),
                        name
                    ),
                    style,
                )
            }
        })
        .collect();
    render_rows(f, inner, &rows, st.edit_idx, &mut st.edit_offset, focused);
}

fn render_pending(f: &mut Frame, st: &mut State, area: Rect) {
    let title = if st.pending.is_empty() {
        "[5] Pending comments".to_string()
    } else {
        format!("[5] Pending comments ({})", st.pending.len())
    };
    let busy = st.busy.contains("review") || st.busy.contains("pending");
    let b = block(&title, st.focus == Focus::Pending, busy);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let focused = st.focus == Focus::Pending;
    if st.pending.is_empty() {
        let msg = if is_loading(st) { "Loading…" } else { "No pending comments" };
        f.render_widget(Paragraph::new(Line::styled(msg, theme::dim())), inner);
        return;
    }
    // Group comments into a file tree; each file's comments hang under it.
    let mut paths: Vec<String> = st.pending.iter().map(|c| c.path.clone()).collect();
    paths.sort();
    paths.dedup();
    let tree = crate::tree::build_tree_from_paths(&paths, &HashSet::new());
    // rows: (text, style, Some(comment_idx) for selectable leaves)
    let mut rows: Vec<(String, Style, Option<usize>)> = Vec::new();
    for row in &tree {
        match row {
            TreeRow::Dir { depth, name, .. } => rows.push((
                format!("{}{}/", "  ".repeat(*depth), name),
                Style::default().add_modifier(Modifier::BOLD),
                None,
            )),
            TreeRow::File { depth, name, index } => {
                rows.push((
                    format!("{}{}", "  ".repeat(*depth), name),
                    theme::title(),
                    None,
                ));
                let path = &paths[*index];
                for (ci, c) in st.pending.iter().enumerate() {
                    if &c.path != path {
                        continue;
                    }
                    let first = c.body.lines().next().unwrap_or("");
                    let loc = match c.start_line {
                        Some(s) => format!("{}-{}", s.min(c.line), s.max(c.line)),
                        None => c.line.to_string(),
                    };
                    rows.push((
                        format!("{}{loc}: {first}", "  ".repeat(depth + 1)),
                        Style::default(),
                        Some(ci),
                    ));
                }
            }
        }
    }
    let sel_row = if focused {
        rows.iter().position(|(_, _, ci)| *ci == Some(st.pending_idx)).unwrap_or(0)
    } else {
        st.pending_offset
    };
    let (vh, iw) = (inner.height as usize, inner.width as usize);
    st.pending_offset = clamp_view(sel_row, st.pending_offset, vh, rows.len());
    let mut lines = Vec::new();
    for (text, style, ci) in rows.iter().skip(st.pending_offset).take(vh) {
        let selected = focused && ci.is_some() && *ci == Some(st.pending_idx);
        lines.push(list_row(selected, text, *style, iw));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Rows for one column of a split local diff: plain diff lines, the selected
/// change block marked (`cur`, active column only) and the comment-picker
/// selection reversed.
fn diff_column_rows(
    lines: &[String],
    scroll: usize,
    vh: usize,
    tw: usize,
    cur: Option<(usize, usize)>,
    sel: (usize, usize),
) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = Vec::new();
    let mut i = scroll;
    while out.len() < vh && i < lines.len() {
        let ln = &lines[i];
        let current = cur.map_or(false, |(s, e)| s <= i && i < e);
        let selected = sel.0 <= i && i <= sel.1;
        let mut style = theme::diff_line_style(ln, current);
        if selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        let base_marker = if selected {
            "▶"
        } else if current {
            "▌"
        } else {
            " "
        };
        let m_style = if selected {
            theme::focus()
        } else if current {
            theme::hunk_marker()
        } else {
            Style::default()
        };
        for (k, chunk) in wrap_hard(&ln.replace('\t', "    "), tw.max(1)).into_iter().enumerate() {
            if out.len() >= vh {
                break;
            }
            let marker = if k > 0 { " " } else { base_marker };
            out.push(Line::from(vec![
                Span::styled(marker, m_style),
                Span::styled(pad(&chunk, tw), style),
            ]));
        }
        i += 1;
    }
    out
}

/// A partly-staged file's local diff, side by side: unstaged left, staged right.
/// The focused column drives j/k and Space; `h`/`l` moves between them.
fn render_split_diff(f: &mut Frame, st: &mut State, inner: Rect, path: &str) {
    let cols = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(inner);
    let (sel_lo, sel_hi) = if st.comment_mode {
        let anchor = st.comment_start.unwrap_or(st.comment_line);
        (anchor.min(st.comment_line), anchor.max(st.comment_line))
    } else {
        (1usize, 0usize) // empty
    };
    let body_h = inner.height.saturating_sub(1) as usize;
    // Reveal the active column's selected block before splitting the scrolls.
    if st.diff_reveal_pending {
        let range = if st.comment_mode {
            Some((st.comment_line, st.comment_line + 1))
        } else {
            current_hunk_range(st, path)
        };
        if let Some((s, e)) = range {
            st.diff_scroll = reveal_scroll(st.diff_scroll, s, e, body_h);
        }
        st.diff_reveal_pending = false;
    }
    let cur = if st.focus == Focus::Diff { current_hunk_range(st, path) } else { None };

    for (staged, area) in [(false, cols[0]), (true, cols[2])] {
        let active = staged == st.staged_side;
        let (diffs, _, hunks) = if staged {
            (&st.staged_diff_by_file, &st.staged_info_by_file, &st.staged_hunks_by_file)
        } else {
            (&st.unstaged_diff_by_file, &st.unstaged_info_by_file, &st.unstaged_hunks_by_file)
        };
        let empty = Vec::new();
        let lines = diffs.get(path).unwrap_or(&empty);
        let n_hunks = hunks.get(path).map_or(0, |h| h.len());
        let label = format!(
            "{} {} ({n_hunks})",
            if active { "▌" } else { " " },
            if staged { "Staged" } else { "Unstaged" }
        );
        let (lstyle, tw) = (
            if active { theme::focus() } else { theme::dim() },
            area.width.saturating_sub(1) as usize,
        );
        f.render_widget(
            Paragraph::new(Line::styled(pad(&label, area.width as usize), lstyle)),
            Rect { height: 1, ..area },
        );
        let body = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
        let scroll = if active { &mut st.diff_scroll } else { &mut st.alt_diff_view.0 };
        *scroll = (*scroll).min(lines.len().saturating_sub(1));
        let rows = diff_column_rows(
            lines,
            *scroll,
            body.height as usize,
            tw,
            if active { cur } else { None },
            if active { (sel_lo, sel_hi) } else { (1, 0) },
        );
        f.render_widget(Paragraph::new(rows), body);
    }
    // Separator between the two columns.
    let sep: Vec<Line> = (0..inner.height).map(|_| Line::styled("│", theme::dim())).collect();
    f.render_widget(Paragraph::new(sep), cols[1]);
}

fn render_diff(f: &mut Frame, st: &mut State, area: Rect) {
    let path = diff_path(st);
    let local = path.as_ref().map_or(false, |p| is_local_diff(st, p));
    let has_overlay = !local && path.as_ref().map_or(false, |p| st.edit_diff_by_file.contains_key(p));
    let split = local && path.as_ref().map_or(false, |p| is_split(st, p));
    let title = match (&path, local) {
        (Some(p), true) => format!("[0] Local diff — {p}{}", if split { "  · unstaged | staged" } else { "" }),
        (Some(p), false) => format!("[0] Diff — {p}{}", if has_overlay { "  · +local edits" } else { "" }),
        (None, _) => "[0] Diff".to_string(),
    };
    let b = block(&title, st.focus == Focus::Diff, false);
    let inner = b.inner(area);
    f.render_widget(b, area);
    if split {
        let p = path.unwrap();
        render_split_diff(f, st, inner, &p);
        return;
    }
    let (vh, iw) = (inner.height as usize, inner.width as usize);

    if is_loading(st) && st.diff_by_file.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("Loading…", theme::dim())), inner);
        return;
    }

    let focused = st.focus == Focus::Diff;
    let cur_hr = if focused { path.as_ref().and_then(|p| current_hunk_range(st, p)) } else { None };
    // Only recenter after a keyboard navigation; mouse/PgUp/Dn scroll freely.
    if st.diff_reveal_pending {
        if st.comment_mode {
            st.diff_scroll = reveal_scroll(st.diff_scroll, st.comment_line, st.comment_line + 1, vh);
        } else if let Some((s, e)) = cur_hr {
            st.diff_scroll = reveal_scroll(st.diff_scroll, s, e, vh);
        }
        st.diff_reveal_pending = false;
    }
    // Scroll is by diff-line index; allow reaching the last line (which may wrap
    // into several rows) rather than clamping to len - vh.
    let n_lines = path.as_ref().and_then(|p| source_maps(st, p).0.get(p)).map_or(0, Vec::len);
    st.diff_scroll = st.diff_scroll.min(n_lines.saturating_sub(1));

    let empty = Vec::new();
    let lines_vec = path.as_ref().and_then(|p| source_maps(st, p).0.get(p)).unwrap_or(&empty);
    let placeholder;
    let diff_lines: &[String] = if lines_vec.is_empty() && path.is_some() {
        placeholder = vec!["(no diff — binary, removed, or too large)".to_string()];
        &placeholder
    } else {
        lines_vec
    };

    let (sel_lo, sel_hi) = if st.comment_mode {
        let anchor = st.comment_start.unwrap_or(st.comment_line);
        (anchor.min(st.comment_line), anchor.max(st.comment_line))
    } else {
        (1usize, 0usize) // empty
    };

    // Pending comments to show inline (matched to the shown diff via info_here).
    let pending_here: Vec<&PendingComment> = match &path {
        Some(p) => st.pending.iter().filter(|c| &c.path == p).collect(),
        None => Vec::new(),
    };
    let info_here = path.as_ref().and_then(|p| source_maps(st, p).1.get(p));

    // Local (uncommitted) worktree edits to overlay in orange on the PR diff. The
    // local diff already *is* the edits, so no overlay there.
    let overlay = if local {
        None
    } else {
        path.as_ref().and_then(|p| {
            match (st.edit_diff_by_file.get(p), st.edit_info_by_file.get(p)) {
                (Some(l), Some(inf)) if !l.is_empty() => Some(crate::diff::local_overlay(l, inf)),
                _ => None,
            }
        })
    };

    // PR-deleted line contents: a locally re-added line matching one is a
    // *restore* (shown like context, leading space) rather than a new `+` line.
    let pr_deleted: HashSet<&str> = diff_lines
        .iter()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
        .map(|l| &l[1..])
        .collect();

    let tw = iw.saturating_sub(1); // text width (1 col for the marker)
    // Emit local additions (orange, "▎" marker) — as many wrapped rows as fit.
    // Keep the diff column aligned: real additions/edits get a `+`, a restored
    // line gets a space so it lines up with the surrounding context.
    let push_adds = |out: &mut Vec<Line>, adds: &[String]| {
        for add in adds {
            let prefix = if pr_deleted.contains(add.as_str()) { ' ' } else { '+' };
            let shown = format!("{prefix}{}", add.replace('\t', "    "));
            for (k, chunk) in wrap_hard(&shown, tw).into_iter().enumerate() {
                if out.len() >= vh {
                    return;
                }
                let marker = if k > 0 { " " } else { "▎" };
                out.push(Line::from(vec![
                    Span::styled(marker, theme::local_marker()),
                    Span::styled(pad(&chunk, tw), theme::local_add()),
                ]));
            }
        }
    };

    let mut out: Vec<Line> = Vec::new();
    let mut top_done = false;
    let mut i = st.diff_scroll;
    while out.len() < vh && i < diff_lines.len() {
        let ln = &diff_lines[i];
        let new_side = info_here.and_then(|info| info.get(i)).and_then(|&(_, n)| n);
        let current = cur_hr.map_or(false, |(s, e)| s <= i && i < e);
        let selected = sel_lo <= i && i <= sel_hi;
        // A head line removed locally: draw it struck-through in orange.
        let local_del = overlay
            .as_ref()
            .zip(new_side)
            .map_or(false, |(ov, l)| ov.deleted_heads.contains(&l));

        // Local additions anchored before the first head line show once, up top.
        if let (Some(ov), false) = (&overlay, top_done) {
            if new_side.is_some() {
                push_adds(&mut out, &ov.adds_top);
                top_done = true;
            }
        }

        let mut style = theme::diff_line_style(ln, current);
        if local_del {
            style = theme::local_del();
        }
        if selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        let base_marker = if selected { "▶" } else if local_del { "▎" } else if current { "▌" } else { " " };
        let m_style = if selected {
            theme::focus()
        } else if local_del {
            theme::local_marker()
        } else if current {
            theme::hunk_marker()
        } else {
            Style::default()
        };
        // Wrap long lines onto continuation rows so nothing is cut off.
        for (k, chunk) in wrap_hard(&ln.replace('\t', "    "), tw).into_iter().enumerate() {
            if out.len() >= vh {
                break;
            }
            let marker = if k > 0 { " " } else { base_marker };
            out.push(Line::from(vec![Span::styled(marker, m_style), Span::styled(pad(&chunk, tw), style)]));
        }
        // Local additions inserted after this head line (orange).
        if let (Some(ov), Some(l)) = (&overlay, new_side) {
            if let Some(adds) = ov.adds_after.get(&l) {
                push_adds(&mut out, adds);
            }
        }
        // Inline any pending comment anchored to this line.
        if let Some(&(old, new)) = info_here.and_then(|info| info.get(i)) {
            for c in &pending_here {
                let hit = if c.side == "LEFT" { old == Some(c.line) } else { new == Some(c.line) };
                if !hit {
                    continue;
                }
                let range_tag = match c.start_line {
                    Some(s) => format!("[{}-{}] ", s.min(c.line), s.max(c.line)),
                    None => String::new(),
                };
                for (bi, bl) in c.body.lines().enumerate() {
                    if out.len() >= vh {
                        break;
                    }
                    let gutter = if bi == 0 { "▏💬 " } else { "▏   " };
                    let tag = if bi == 0 { range_tag.as_str() } else { "" };
                    let text = format!("{gutter}{tag}{}", bl.replace('\t', "    "));
                    out.push(Line::from(Span::styled(pad(&text, iw), theme::comment_inline())));
                }
            }
        }
        i += 1;
    }
    f.render_widget(Paragraph::new(out), inner);
}

fn render_pr_details(f: &mut Frame, st: &mut State, area: Rect) {
    let pr = st.prs.get(st.pr_idx);
    let (title, data) = match pr {
        Some(p) => (format!("PR #{} · {}", p.number, p.title), st.pr_details.get(&p.number)),
        None => ("PR details".to_string(), None),
    };
    let busy = matches!(data, Some(None));
    let b = block(&title, false, busy);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let (vh, iw) = (inner.height as usize, inner.width as usize);
    match data {
        Some(Some(v)) => {
            let lines = wrap_styled(format_pr_details(v), iw.saturating_sub(1));
            st.details_scroll = st.details_scroll.min(lines.len().saturating_sub(vh));
            let out: Vec<Line> = lines
                .iter()
                .skip(st.details_scroll)
                .take(vh)
                .map(|(t, k)| Line::styled(t.replace('\t', "    "), theme::kind_style(*k)))
                .collect();
            f.render_widget(Paragraph::new(out), inner);
        }
        _ => f.render_widget(Paragraph::new(Line::styled("Loading…", theme::dim())), inner),
    }
}

fn render_commit_detail(f: &mut Frame, st: &State, area: Rect) {
    // Not focused: the focus is on the list this previews, and two lit borders
    // read as two places the keys apply.
    let b = block("Commit", false, false);
    let inner = b.inner(area);
    f.render_widget(b, area);
    if st.commits.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("No commits", theme::dim())), inner);
        return;
    }
    let c = &st.commits[st.commit_idx.min(st.commits.len() - 1)];
    let mut out = vec![
        Line::styled(format!("{}  {}", c.short(), c.headline), theme::title()),
        Line::styled(format!("{}   {}", c.author, c.date), theme::dim()),
        Line::from(""),
    ];
    for bl in c.body.lines() {
        out.push(Line::from(bl.to_string()));
    }
    f.render_widget(Paragraph::new(out), inner);
}

fn render_pending_detail(f: &mut Frame, st: &State, area: Rect) {
    let b = block("Pending comment", false, false);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let iw = inner.width as usize;
    if st.pending.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("No pending comments", theme::dim())), inner);
        return;
    }
    let c = &st.pending[st.pending_idx.min(st.pending.len() - 1)];
    let mut out = vec![Line::styled(format!("{}:{}", c.path, c.line), theme::title())];
    let (hunk_lines, target) = hunk_for_comment(st, c);
    if hunk_lines.is_empty() {
        out.push(Line::styled("(hunk not in current diff)", theme::dim()));
    } else {
        for (j, ln) in hunk_lines.iter().enumerate() {
            let is_t = target == Some(j);
            let marker = if is_t { "▌" } else { " " };
            out.push(Line::from(vec![
                Span::styled(marker, theme::comment_marker()),
                Span::styled(ln.replace('\t', "    "), theme::diff_line_style(ln, false)),
            ]));
        }
    }
    out.push(Line::styled("─".repeat(iw.saturating_sub(1)), theme::dim()));
    out.push(Line::styled("Comment:", theme::focus()));
    for bl in c.body.lines() {
        out.push(Line::from(bl.to_string()));
    }
    f.render_widget(Paragraph::new(out), inner);
}

/// The right pane while the Pending-edits pane is focused: the local diff of the
/// selected changed file (exactly what will be committed).
///
/// Named and framed as `[0]`, like every other thing that pane can show: it
/// carried `[4] Pending edits` and the focus border, so two panes claimed the
/// same number and the same focus at once. It is the same content hunk mode
/// shows, so it wears the same name.
fn render_edit_diff(f: &mut Frame, st: &mut State, area: Rect) {
    let path = match st.edit_tree.get(st.edit_idx) {
        Some(TreeRow::File { index, .. }) => st.edit_files.get(*index).map(|e| e.path.clone()),
        _ => None,
    };
    let title = match &path {
        Some(p) => format!("[0] Local diff — {p}"),
        None => "[0] Local diff".to_string(),
    };
    let b = block(&title, false, false);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let (vh, iw) = (inner.height as usize, inner.width as usize);
    let tw = iw.saturating_sub(1);

    let empty = Vec::new();
    let lines_vec = path.as_ref().and_then(|p| st.edit_diff_by_file.get(p)).unwrap_or(&empty);
    if lines_vec.is_empty() {
        let msg = if path.is_some() { "(no diff)" } else { "Select a changed file (or edit one with 'e')" };
        f.render_widget(Paragraph::new(Line::styled(msg, theme::dim())), inner);
        return;
    }
    st.edit_diff_scroll = st.edit_diff_scroll.min(lines_vec.len().saturating_sub(1));
    let mut out: Vec<Line> = Vec::new();
    let mut i = st.edit_diff_scroll;
    while out.len() < vh && i < lines_vec.len() {
        let ln = &lines_vec[i];
        let style = theme::diff_line_style(ln, false);
        for chunk in wrap_hard(&ln.replace('\t', "    "), tw.max(1)) {
            if out.len() >= vh {
                break;
            }
            out.push(Line::from(Span::styled(pad(&chunk, tw), style)));
        }
        i += 1;
    }
    f.render_widget(Paragraph::new(out), inner);
}

// ---- overlays ----

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn draw_editor(f: &mut Frame, ta: &textbuffer::TextArea, inner: Rect, help: &str) {
    let editor_h = inner.height.saturating_sub(1) as usize;
    let editor_w = inner.width as usize;
    let (visual, cur_vrow, cur_vcol) = textbuffer::wrap(ta, editor_w.max(1));
    let first = cur_vrow.saturating_sub(editor_h.saturating_sub(1));
    let mut out = Vec::new();
    for (_, _, text) in visual.iter().skip(first).take(editor_h) {
        out.push(Line::from(text.clone()));
    }
    f.render_widget(Paragraph::new(out), Rect { height: inner.height.saturating_sub(1), ..inner });
    // help line at the bottom of the modal
    let help_y = inner.y + inner.height.saturating_sub(1);
    f.render_widget(
        Paragraph::new(help.to_string()).style(theme::keys()),
        Rect { x: inner.x, y: help_y, width: inner.width, height: 1 },
    );
    // cursor
    let cx = inner.x + (cur_vcol as u16).min(inner.width.saturating_sub(1));
    let cy = inner.y + (cur_vrow.saturating_sub(first)) as u16;
    if cy < help_y {
        f.set_cursor_position((cx, cy));
    }
}

/// The shared comment-editor modal (used by both new-comment and edit-comment):
/// a centered box with a title, the wrapped text editor and a shortcuts line.
fn draw_modal_editor(f: &mut Frame, area: Rect, ta: &textbuffer::TextArea, title: &str, help: &str) {
    let rect = centered(area, 120.min(area.width.saturating_sub(4)), 18.min(area.height.saturating_sub(4)));
    let b = block(title, true, false);
    let inner = b.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(b, rect);
    draw_editor(f, ta, inner, help);
}

/// A commit's hooks while they run, and what they said if they refused.
///
/// Tall and wide: hook output is the kind of thing you scroll back through, and
/// a five-line window turns a stack trace into a puzzle. Red when it failed —
/// that is the whole signal, since a passing run closes itself.
fn render_hooks(f: &mut Frame, area: Rect, title: &str, lines: &[String], failed: bool, scroll: usize) {
    let rect = centered(area, 100.min(area.width.saturating_sub(4)), 26.min(area.height.saturating_sub(2)));
    let bs = if failed { theme::hook_failed() } else { theme::border_focused() };
    let label = format!(" {title}{} ", if failed { "" } else { " ⏳" });
    let b = Block::default()
        .borders(Borders::ALL)
        .border_style(bs)
        .title(Span::styled(label, bs));
    let inner = b.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(b, rect);

    let body_h = inner.height.saturating_sub(1) as usize;
    // Follows the tail while it runs; a failure stops so the error stays put.
    let start = if failed { scroll.min(lines.len().saturating_sub(1)) } else { lines.len().saturating_sub(body_h) };
    let shown: Vec<Line> = lines
        .iter()
        .skip(start)
        .take(body_h)
        .map(|l| Line::from(Span::raw(l.clone())))
        .collect();
    f.render_widget(Paragraph::new(shown), Rect { height: body_h as u16, ..inner });

    let help = if failed { "j/k: scroll · Esc: close" } else { "running… · Esc: hide (they carry on)" };
    let help_y = inner.y + inner.height.saturating_sub(1);
    f.render_widget(
        Paragraph::new(help).style(theme::keys()),
        Rect { x: inner.x, y: help_y, width: inner.width, height: 1 },
    );
}

fn render_overlay(f: &mut Frame, st: &State) {
    let area = f.area();
    match &st.overlay {
        Overlay::None => {}
        Overlay::Comment { ta, path, line, side, start_line, .. } => {
            let where_ = match start_line {
                Some(s) => format!("{path}:{s}-{line}"),
                None => format!("{path}:{line} ({side})"),
            };
            draw_modal_editor(f, area, ta, &format!("Comment on {where_}"),
                "Enter: add · Alt+Enter: newline · Ctrl+S: suggestion · Ctrl+Bksp: del word · Esc: back");
        }
        Overlay::Edit { ta, path, line, .. } => {
            draw_modal_editor(f, area, ta, &format!("Edit comment on {path}:{line}"),
                "Enter: save · Alt+Enter: newline · Ctrl+S: suggestion · Ctrl+D: delete · Esc: cancel");
        }
        Overlay::CommitMsg { ta, kind } => {
            let n = st.edit_files.len();
            let what = match kind {
                CommitKind::Amend => format!("Amend HEAD with {n} file(s)"),
                CommitKind::NoVerify => format!("Commit {n} file(s) locally · no hooks"),
                CommitKind::New => format!("Commit {n} file(s) locally"),
            };
            draw_modal_editor(f, area, ta, &what,
                "Enter: commit · Alt+Enter: newline · Ctrl+Bksp: del word · Esc: cancel");
        }
        Overlay::Hooks { title, lines, failed, scroll } => {
            render_hooks(f, area, title, lines, *failed, *scroll);
        }
        Overlay::Ask { ta } => {
            draw_modal_editor(f, area, ta, "Ask Claude about this hunk",
                "Enter: launch Claude · Alt+Enter: newline · Ctrl+Bksp: del word · Esc: cancel");
        }
        Overlay::Confirm { prompt, .. } => {
            let w = (prompt.chars().count() as u16 + 6).clamp(30, area.width.saturating_sub(4));
            let rect = centered(area, w, 5);
            let b = block("Confirm", true, false);
            let inner = b.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(b, rect);
            f.render_widget(Paragraph::new(prompt.clone()), inner);
            let help_y = inner.y + inner.height.saturating_sub(1);
            f.render_widget(
                Paragraph::new("y: confirm · any other key: cancel").style(theme::keys()),
                Rect { x: inner.x, y: help_y, width: inner.width, height: 1 },
            );
        }
        Overlay::Review { ta, editing, choice } => {
            let rect = centered(area, 90.min(area.width.saturating_sub(4)), 22.min(area.height.saturating_sub(4)));
            let title = format!("Finish review · {} pending comment{}", st.pending.len(), if st.pending.len() == 1 { "" } else { "s" });
            let b = block(&title, true, false);
            let inner = b.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(b, rect);
            let choices_h = SUBMIT_CHOICES.len() as u16;
            let editor_h = inner.height.saturating_sub(choices_h + 2);
            // editor
            let mut ed = Vec::new();
            for l in ta.lines.iter() {
                ed.push(Line::from(l.clone()));
            }
            f.render_widget(Paragraph::new(ed), Rect { height: editor_h, ..inner });
            // divider
            let div_y = inner.y + editor_h;
            f.render_widget(Paragraph::new(Line::styled("─".repeat(inner.width as usize), theme::dim())),
                Rect { x: inner.x, y: div_y, width: inner.width, height: 1 });
            // choices
            for (i, (_, label)) in SUBMIT_CHOICES.iter().enumerate() {
                let focused_choice = !*editing && i == *choice;
                let style = if focused_choice { theme::selection() } else if !*editing { theme::focus() } else { theme::dim() };
                let marker = if focused_choice { "▸ " } else { "  " };
                f.render_widget(
                    Paragraph::new(Line::styled(pad(&format!("{marker}{label}"), inner.width as usize), style)),
                    Rect { x: inner.x, y: div_y + 1 + i as u16, width: inner.width, height: 1 },
                );
            }
            let help = if *editing {
                "Shift+Enter: newline · Enter: choose event · Esc: cancel"
            } else {
                "j/k: select · Enter: submit · k at top: back · Esc: cancel"
            };
            let help_y = inner.y + inner.height.saturating_sub(1);
            f.render_widget(Paragraph::new(help.to_string()).style(theme::keys()),
                Rect { x: inner.x, y: help_y, width: inner.width, height: 1 });
            if *editing {
                let (_, cur_vrow, cur_vcol) = textbuffer::wrap(ta, inner.width.max(1) as usize);
                let cy = inner.y + cur_vrow.min(editor_h.saturating_sub(1) as usize) as u16;
                let cx = inner.x + (cur_vcol as u16).min(inner.width.saturating_sub(1));
                f.set_cursor_position((cx, cy));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_selection_visible() {
        assert_eq!(clamp_view(0, 5, 10, 100), 0);
        assert_eq!(clamp_view(20, 0, 10, 100), 11);
        assert_eq!(clamp_view(3, 0, 10, 100), 0);
        assert_eq!(clamp_view(0, 0, 10, 0), 0);
    }

    #[test]
    fn wrap_indented_hangs_continuation() {
        let out = wrap_indented("● #42 ", "alpha beta gamma delta", 14);
        // First line carries the prefix; continuations align under the title.
        assert_eq!(out[0], "● #42 alpha");
        assert!(out.len() >= 2);
        for l in &out[1..] {
            assert!(l.starts_with("      "), "continuation should be indented: {l:?}");
        }
        // Nothing exceeds the width.
        assert!(out.iter().all(|l| l.chars().count() <= 14));
    }

    #[test]
    fn wrap_indented_hard_splits_long_word() {
        let out = wrap_indented("x ", "supercalifragilistic", 8);
        assert!(out.iter().all(|l| l.chars().count() <= 8));
        assert_eq!(out[0], "x superc");
    }

    #[test]
    fn wrap_hard_splits_long_lines() {
        assert_eq!(wrap_hard("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(wrap_hard("abc", 3), ["abc"]);
        assert_eq!(wrap_hard("", 5), [""]);
    }

    #[test]
    fn reveal_minimal() {
        assert_eq!(reveal_scroll(8, 10, 13, 10), 8); // visible → unchanged
        assert_eq!(reveal_scroll(20, 5, 8, 10), 5); // above → to it
        assert_eq!(reveal_scroll(0, 15, 18, 10), 8); // below → end at bottom
        assert_eq!(reveal_scroll(0, 5, 40, 10), 5); // taller than viewport → top
    }

    #[test]
    fn pr_rows_groups_with_headers() {
        use crate::models::{Category, Pr};
        let mut st = State::default();
        st.prs = vec![
            Pr { number: 3, title: "a".into(), head: "h".into(), author: "me".into(), node_id: String::new(), category: Category::Mine, created_at: String::new(), updated_at: String::new() },
            Pr { number: 5, title: "b".into(), head: "h".into(), author: "x".into(), node_id: String::new(), category: Category::Review, created_at: String::new(), updated_at: String::new() },
        ];
        let rows = pr_rows(&st);
        assert!(rows[0].0 && rows[0].1.starts_with("My PRs"));
        assert!(!rows[1].0 && rows[1].2 == 0);
        assert!(rows[2].0 && rows[2].1.starts_with("Requested review"));
        assert!(!rows[3].0 && rows[3].2 == 1);
    }
}
