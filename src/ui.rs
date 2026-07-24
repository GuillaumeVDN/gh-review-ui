//! Ratatui rendering of the panes + modal overlays.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::markdown::{format_pr_details, wrap_styled};
use crate::models::{Focus, Overlay, State, TreeRow, REVIEW_EVENTS};
use crate::navigation::{cur_file_path, current_hunk_range, hunk_for_comment};
use crate::textbuffer;
use crate::theme;

/// Rectangles of the four left panes + the right pane (for mouse hit-testing).
pub struct PaneRects {
    pub prs: Rect,
    pub commits: Rect,
    pub files: Rect,
    pub pending: Rect,
    pub right: Rect,
    pub body: Rect,
}

pub fn compute_layout(area: Rect) -> (PaneRects, Rect, Rect) {
    let root = Layout::vertical([Constraint::Min(0), Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    let (body, status, help) = (root[0], root[1], root[2]);
    let left_w = (area.width / 3).max(38).min(area.width.saturating_sub(20));
    let cols = Layout::horizontal([Constraint::Length(left_w), Constraint::Min(0)]).split(body);
    let (left, right) = (cols[0], cols[1]);
    let bh = body.height;
    let pr_h = (bh * 2 / 9).max(6);
    let commits_h = (bh / 6).max(4);
    let pending_h = (bh / 8).max(3);
    let rows = Layout::vertical([
        Constraint::Length(pr_h),
        Constraint::Length(commits_h),
        Constraint::Min(3),
        Constraint::Length(pending_h),
    ])
    .split(left);
    (
        PaneRects { prs: rows[0], commits: rows[1], files: rows[2], pending: rows[3], right, body },
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

/// Section rows for the PRs pane: header labels + PR indices.
pub fn pr_rows(st: &State) -> Vec<(bool, String, usize)> {
    // (is_header, text, pr_index)
    use crate::models::Category;
    let mut rows = Vec::new();
    let mut last: Option<Category> = None;
    for (i, pr) in st.prs.iter().enumerate() {
        if last != Some(pr.category) {
            let (label, cat) = match pr.category {
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
    let (rects, status_area, help_area) = compute_layout(f.area());
    render_prs(f, st, rects.prs);
    render_commits(f, st, rects.commits);
    render_files(f, st, rects.files);
    render_pending(f, st, rects.pending);
    match st.focus {
        Focus::Prs => render_pr_details(f, st, rects.right),
        Focus::Commits => render_commit_detail(f, st, rects.right),
        Focus::Pending => render_pending_detail(f, st, rects.right),
        _ => render_diff(f, st, rects.right),
    }

    let status = if st.status.is_empty() { "Ready." } else { &st.status };
    f.render_widget(Paragraph::new(status.to_string()).style(theme::status()), status_area);
    f.render_widget(Paragraph::new(shortcuts_for(st)).style(theme::keys()), help_area);

    render_overlay(f, st);
}

fn shortcuts_for(st: &State) -> String {
    let common = "0-4: focus · Tab: next · f: finish review · r: refresh · q: quit";
    match st.focus {
        Focus::Prs => format!("Enter: open (worktree) · j/k: move · Shift+J/K: scroll summary · {common}"),
        Focus::Commits => format!("Space: toggle · a: all/none · Enter: apply range · j/k: move · {common}"),
        Focus::Pending => format!("Enter: submit review · e: edit · d: delete · j/k: move · {common}"),
        Focus::Files => format!("Enter: open/collapse · Space: viewed · e: editor · z: fold · j/k: move · {common}"),
        Focus::Diff => format!("j/k: next/prev block · c: comment · e: editor · PgUp/Dn: scroll · Esc: back · {common}"),
    }
}

fn render_prs(f: &mut Frame, st: &mut State, area: Rect) {
    let title = format!("[1] PRs [{}/{}]", st.repo_owner, st.repo_name);
    let b = block(&title, st.focus == Focus::Prs, st.busy.contains("prs"));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let (vh, iw) = (inner.height as usize, inner.width as usize);
    let rows = pr_rows(st);
    let sel_row = rows.iter().position(|(h, _, i)| !h && *i == st.pr_idx).unwrap_or(0);
    st.pr_offset = clamp_view(sel_row, st.pr_offset, vh, rows.len());
    let mut lines = Vec::new();
    for (is_hdr, text, idx) in rows.iter().skip(st.pr_offset).take(vh) {
        if *is_hdr {
            lines.push(Line::styled(pad(text, iw), theme::section_header()));
            continue;
        }
        let pr = &st.prs[*idx];
        let active = st.active_pr.as_ref().map_or(false, |a| a.number == pr.number);
        let text = format!("{}#{} {}", if active { "● " } else { "  " }, pr.number, pr.title);
        let style = if *idx == st.pr_idx && st.focus == Focus::Prs {
            theme::selection()
        } else if active {
            theme::active_pr()
        } else {
            Style::default()
        };
        lines.push(Line::styled(pad(&text, iw), style));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_commits(f: &mut Frame, st: &mut State, area: Rect) {
    let title = if st.commits.is_empty() {
        "[2] Commits".to_string()
    } else {
        format!("[2] Commits ({}/{})", st.commit_selected.len(), st.commits.len())
    };
    let busy = st.busy.contains("active") || st.busy.contains("commitdiff");
    let b = block(&title, st.focus == Focus::Commits, busy);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let (vh, iw) = (inner.height as usize, inner.width as usize);
    if st.commits.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("No commits", theme::dim())), inner);
        return;
    }
    st.commit_offset = clamp_view(st.commit_idx, st.commit_offset, vh, st.commits.len());
    let mut lines = Vec::new();
    for (i, c) in st.commits.iter().enumerate().skip(st.commit_offset).take(vh) {
        let checked = st.commit_selected.contains(&c.oid);
        let text = format!("[{}] {} {}", if checked { "x" } else { " " }, c.short(), c.headline);
        let style = if i == st.commit_idx && st.focus == Focus::Commits {
            theme::selection()
        } else if !checked {
            theme::dim()
        } else {
            Style::default()
        };
        lines.push(Line::styled(pad(&text, iw), style));
    }
    f.render_widget(Paragraph::new(lines), inner);
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
    let (vh, iw) = (inner.height as usize, inner.width as usize);
    st.file_offset = clamp_view(st.file_idx, st.file_offset, vh, st.tree.len());
    let mut lines = Vec::new();
    for (i, row) in st.tree.iter().enumerate().skip(st.file_offset).take(vh) {
        let (text, base) = match row {
            TreeRow::Dir { depth, name, collapsed, .. } => (
                format!("{}{} {}/", "  ".repeat(*depth), if *collapsed { "▶" } else { "▼" }, name),
                Style::default().add_modifier(ratatui::style::Modifier::BOLD),
            ),
            TreeRow::File { depth, name, index } => {
                let viewed = st.files[*index].viewed;
                (
                    format!("{}[{}] {}", "  ".repeat(*depth), if viewed { "✔" } else { " " }, name),
                    if viewed { theme::dim() } else { Style::default() },
                )
            }
        };
        let style = if i == st.file_idx && st.focus == Focus::Files { theme::selection() } else { base };
        lines.push(Line::styled(pad(&text, iw), style));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_pending(f: &mut Frame, st: &mut State, area: Rect) {
    let title = format!("[4] Pending ({})", st.pending.len());
    let busy = st.busy.contains("review") || st.busy.contains("pending");
    let b = block(&title, st.focus == Focus::Pending, busy);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let (vh, iw) = (inner.height as usize, inner.width as usize);
    if st.pending.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("No pending comments", theme::dim())), inner);
        return;
    }
    st.pending_offset = clamp_view(st.pending_idx, st.pending_offset, vh, st.pending.len());
    let mut lines = Vec::new();
    for (i, c) in st.pending.iter().enumerate().skip(st.pending_offset).take(vh) {
        let first = c.body.lines().next().unwrap_or("");
        let text = format!("{}:{}  {}", c.path, c.line, first);
        let style = if i == st.pending_idx && st.focus == Focus::Pending { theme::selection() } else { Style::default() };
        lines.push(Line::styled(pad(&text, iw), style));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_diff(f: &mut Frame, st: &mut State, area: Rect) {
    let path = cur_file_path(st);
    let title = match &path {
        Some(p) => format!("[0] Diff — {p}"),
        None => "[0] Diff".to_string(),
    };
    let b = block(&title, st.focus == Focus::Diff, false);
    let inner = b.inner(area);
    f.render_widget(b, area);
    let (vh, iw) = (inner.height as usize, inner.width as usize);

    let empty = Vec::new();
    let lines_vec = path.as_ref().and_then(|p| st.diff_by_file.get(p)).unwrap_or(&empty);
    let placeholder;
    let diff_lines: &[String] = if lines_vec.is_empty() && path.is_some() {
        placeholder = vec!["(no diff — binary, removed, or too large)".to_string()];
        &placeholder
    } else {
        lines_vec
    };

    let focused = st.focus == Focus::Diff;
    let cur_hr = if focused { path.as_ref().and_then(|p| current_hunk_range(st, p)) } else { None };
    if st.comment_mode {
        st.diff_scroll = reveal_scroll(st.diff_scroll, st.comment_line, st.comment_line + 1, vh);
    } else if let Some((s, e)) = cur_hr {
        st.diff_scroll = reveal_scroll(st.diff_scroll, s, e, vh);
    }
    let max_scroll = diff_lines.len().saturating_sub(vh);
    st.diff_scroll = st.diff_scroll.min(max_scroll);

    let (sel_lo, sel_hi) = if st.comment_mode {
        let anchor = st.comment_start.unwrap_or(st.comment_line);
        (anchor.min(st.comment_line), anchor.max(st.comment_line))
    } else {
        (1usize, 0usize) // empty
    };

    let mut out = Vec::new();
    for (i, ln) in diff_lines.iter().enumerate().skip(st.diff_scroll).take(vh) {
        let current = cur_hr.map_or(false, |(s, e)| s <= i && i < e);
        let selected = sel_lo <= i && i <= sel_hi;
        let mut style = theme::diff_line_style(ln, current);
        if selected {
            style = style.add_modifier(ratatui::style::Modifier::REVERSED);
        }
        let marker = if selected { "▶" } else if current { "▌" } else { " " };
        let m_style = if selected { theme::focus() } else if current { theme::hunk_marker() } else { Style::default() };
        let text = pad(&ln.replace('\t', "    "), iw.saturating_sub(1));
        out.push(Line::from(vec![Span::styled(marker, m_style), Span::styled(text, style)]));
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
    let b = block("Commit", st.focus == Focus::Commits, false);
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
    let b = block("Pending comment", st.focus == Focus::Pending, false);
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

fn render_overlay(f: &mut Frame, st: &State) {
    let area = f.area();
    match &st.overlay {
        Overlay::None => {}
        Overlay::Comment { ta, path, line, side, start_line, .. } => {
            let where_ = match start_line {
                Some(s) => format!("{path}:{s}-{line}"),
                None => format!("{path}:{line} ({side})"),
            };
            let rect = centered(area, 90.min(area.width.saturating_sub(4)), 18.min(area.height.saturating_sub(4)));
            let b = block(&format!("Comment on {where_}"), true, false);
            let inner = b.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(b, rect);
            draw_editor(f, ta, inner, "Enter: add · Shift+Enter: newline · Alt+Bksp: del word · Esc: cancel");
        }
        Overlay::Edit { ta, path, line, .. } => {
            let rect = centered(area, 90.min(area.width.saturating_sub(4)), 18.min(area.height.saturating_sub(4)));
            let b = block(&format!("Edit comment on {path}:{line}"), true, false);
            let inner = b.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(b, rect);
            draw_editor(f, ta, inner, "Enter: save · Shift+Enter: newline · Esc: cancel");
        }
        Overlay::Review { ta, editing, choice } => {
            let rect = centered(area, 90.min(area.width.saturating_sub(4)), 22.min(area.height.saturating_sub(4)));
            let title = format!("Finish review · {} pending comment{}", st.pending.len(), if st.pending.len() == 1 { "" } else { "s" });
            let b = block(&title, true, false);
            let inner = b.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(b, rect);
            let choices_h = REVIEW_EVENTS.len() as u16;
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
            for (i, (_, label)) in REVIEW_EVENTS.iter().enumerate() {
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
            Pr { number: 3, title: "a".into(), head: "h".into(), author: "me".into(), node_id: String::new(), category: Category::Mine },
            Pr { number: 5, title: "b".into(), head: "h".into(), author: "x".into(), node_id: String::new(), category: Category::Review },
        ];
        let rows = pr_rows(&st);
        assert!(rows[0].0 && rows[0].1.starts_with("My PRs"));
        assert!(!rows[1].0 && rows[1].2 == 0);
        assert!(rows[2].0 && rows[2].1.starts_with("Requested review"));
        assert!(!rows[3].0 && rows[3].2 == 1);
    }
}
