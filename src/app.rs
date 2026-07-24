//! Curses/ratatui bootstrap, the event loop and key/mouse dispatch.

use std::io::stdout;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;

use crate::models::{Focus, Overlay, State, TreeRow, REVIEW_EVENTS};
use crate::navigation as nav;
use crate::textbuffer::TextArea;
use crate::worker::{Job, Msg};
use crate::{api, controller, editor, ui, worker};

pub fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    // Best-effort: distinct Shift/Ctrl+Enter, Alt+Backspace on supporting terminals.
    let _ = execute!(
        stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        EnableMouseCapture,
    );
    let result = event_loop(&mut terminal);
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags, DisableMouseCapture);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
    thread::spawn(move || worker::worker_loop(job_rx, msg_tx));

    let mut st = State::default();
    match api::detect_repo() {
        Ok((o, n)) => {
            st.repo_owner = o;
            st.repo_name = n;
        }
        Err(e) => st.status = format!("detect_repo: {e}"),
    }
    st.repo_root = api::get_repo_root();
    st.viewer = api::get_viewer_login();
    if !st.repo_owner.is_empty() {
        controller::submit(&mut st, &job_tx, Job::LoadPrs);
        if let Some(last) = api::load_last_pr(&st.repo_owner, &st.repo_name) {
            st.status = format!("Reopening #{last} from last session…");
            let (repo_root, owner, name) = (st.repo_root.clone(), st.repo_owner.clone(), st.repo_name.clone());
            controller::submit(&mut st, &job_tx, Job::OpenPr { repo_root, owner, name, number: last });
        }
    }

    let mut prev_pr = usize::MAX;
    let mut prev_focus: Option<Focus> = None;
    loop {
        while let Ok(msg) = msg_rx.try_recv() {
            controller::apply_msg(&mut st, msg, &job_tx);
        }
        if st.focus == Focus::Prs {
            if st.pr_idx != prev_pr || prev_focus != Some(Focus::Prs) {
                st.details_scroll = 0;
            }
            controller::maybe_load_details(&mut st, &job_tx);
        }
        prev_pr = st.pr_idx;
        prev_focus = Some(st.focus);

        terminal.draw(|f| ui::render(f, &mut st))?;
        if st.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(80))? {
            let area = size_rect(terminal);
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => handle_key(&mut st, &job_tx, k, area),
                Event::Mouse(m) => handle_mouse(&mut st, m, area),
                _ => {}
            }
        }
    }
    let _ = job_tx.send(Job::Quit);
    Ok(())
}

fn size_rect(terminal: &ratatui::DefaultTerminal) -> Rect {
    terminal
        .size()
        .map(|s| Rect { x: 0, y: 0, width: s.width, height: s.height })
        .unwrap_or(Rect { x: 0, y: 0, width: 80, height: 24 })
}

fn overlay_ta(st: &mut State) -> Option<&mut TextArea> {
    match &mut st.overlay {
        Overlay::Comment { ta, .. } | Overlay::Edit { ta, .. } | Overlay::Review { ta, .. } => Some(ta),
        Overlay::None => None,
    }
}

fn handle_key(st: &mut State, tx: &mpsc::Sender<Job>, k: KeyEvent, area: Rect) {
    if !matches!(st.overlay, Overlay::None) {
        handle_overlay_key(st, tx, k);
        return;
    }
    if st.comment_mode {
        handle_comment_mode(st, tx, k);
        return;
    }
    match k.code {
        KeyCode::Char('q') => st.should_quit = true,
        KeyCode::Char(c @ '0'..='4') => {
            if let Some(f) = Focus::from_digit(c) {
                st.focus = f;
            }
        }
        KeyCode::Char('J') => {
            if st.focus == Focus::Prs {
                st.details_scroll += 1;
            } else {
                nav::scroll_diff(st, 1);
            }
        }
        KeyCode::Char('K') => {
            if st.focus == Focus::Prs {
                st.details_scroll = st.details_scroll.saturating_sub(1);
            } else {
                nav::scroll_diff(st, -1);
            }
        }
        KeyCode::Tab => st.focus = st.focus.next(),
        KeyCode::BackTab => st.focus = st.focus.prev(),
        KeyCode::Char('r') => refresh(st, tx),
        KeyCode::Char('f') => controller::begin_review(st),
        _ => handle_pane_key(st, tx, k, area),
    }
}

fn refresh(st: &mut State, tx: &mpsc::Sender<Job>) {
    if !st.busy.contains("prs") {
        controller::submit(st, tx, Job::LoadPrs);
    }
    if let Some(pr) = st.active_pr.clone() {
        if !st.busy.contains("worktree") && !st.busy.contains("active") {
            let (repo_root, owner, name) = (st.repo_root.clone(), st.repo_owner.clone(), st.repo_name.clone());
            controller::submit(st, tx, Job::OpenPr { repo_root, owner, name, number: pr.number });
        }
    }
    if st.focus == Focus::Prs && !st.prs.is_empty() {
        let n = st.prs[st.pr_idx].number;
        st.pr_details.remove(&n);
        controller::maybe_load_details(st, tx);
    }
    st.status = "Refreshing…".into();
}

fn handle_pane_key(st: &mut State, tx: &mpsc::Sender<Job>, k: KeyEvent, area: Rect) {
    let page = (area.height.saturating_sub(4)).max(1) as usize;
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    match st.focus {
        Focus::Prs => match k.code {
            KeyCode::Down | KeyCode::Char('j') => {
                st.pr_idx = (st.pr_idx + 1).min(st.prs.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => st.pr_idx = st.pr_idx.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char('d') => st.details_scroll += 10,
            KeyCode::PageUp | KeyCode::Char('u') => st.details_scroll = st.details_scroll.saturating_sub(10),
            KeyCode::Enter => {
                if !st.prs.is_empty() && !st.busy.contains("worktree") && !st.busy.contains("active") {
                    let pr = st.prs[st.pr_idx].clone();
                    st.status = format!("Opening #{} in a worktree…", pr.number);
                    let (repo_root, owner, name) = (st.repo_root.clone(), st.repo_owner.clone(), st.repo_name.clone());
                    controller::submit(st, tx, Job::OpenPr { repo_root, owner, name, number: pr.number });
                }
            }
            _ => {}
        },
        Focus::Commits => match k.code {
            KeyCode::Down | KeyCode::Char('j') => {
                st.commit_idx = (st.commit_idx + 1).min(st.commits.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => st.commit_idx = st.commit_idx.saturating_sub(1),
            KeyCode::Char(' ') => toggle_commit(st),
            KeyCode::Char('a') => toggle_all_commits(st),
            KeyCode::Enter => controller::apply_commit_selection(st, tx),
            _ => {}
        },
        Focus::Pending => match k.code {
            KeyCode::Down | KeyCode::Char('j') => {
                st.pending_idx = (st.pending_idx + 1).min(st.pending.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => st.pending_idx = st.pending_idx.saturating_sub(1),
            KeyCode::Enter => controller::begin_review(st),
            KeyCode::Char('e') => {
                if !st.busy.contains("pending") {
                    controller::begin_edit_pending(st);
                }
            }
            KeyCode::Char('d') => discard_pending(st, tx),
            _ => {}
        },
        Focus::Files => match k.code {
            KeyCode::Char('j') if alt => jump_file(st, 1),
            KeyCode::Char('k') if alt => jump_file(st, -1),
            KeyCode::Down | KeyCode::Char('j') => {
                st.file_idx = (st.file_idx + 1).min(st.tree.len().saturating_sub(1));
                st.diff_scroll = 0;
                st.diff_hunk_idx = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                st.file_idx = st.file_idx.saturating_sub(1);
                st.diff_scroll = 0;
                st.diff_hunk_idx = 0;
            }
            KeyCode::Enter => open_file_or_dir(st),
            KeyCode::Char(' ') => controller::mark_viewed(st, tx),
            KeyCode::Char('z') => controller::fold_viewed(st),
            KeyCode::Char('e') => editor::open_current_in_editor(st, true),
            _ => {}
        },
        Focus::Diff => match k.code {
            KeyCode::Down | KeyCode::Char('j') => nav::jump_hunk(st, 1),
            KeyCode::Up | KeyCode::Char('k') => nav::jump_hunk(st, -1),
            KeyCode::PageDown => nav::scroll_diff(st, page as i64),
            KeyCode::PageUp => nav::scroll_diff(st, -(page as i64)),
            KeyCode::Char('c') => controller::enter_comment_mode(st),
            KeyCode::Char('e') => editor::open_current_in_editor(st, false),
            KeyCode::Esc => st.focus = Focus::Files,
            _ => {}
        },
    }
}

fn handle_comment_mode(st: &mut State, tx: &mpsc::Sender<Job>, k: KeyEvent) {
    match k.code {
        KeyCode::Esc => {
            st.comment_mode = false;
            st.status = "Comment cancelled.".into();
        }
        KeyCode::Down | KeyCode::Char('j') => controller::move_comment(st, 1, false),
        KeyCode::Up | KeyCode::Char('k') => controller::move_comment(st, -1, false),
        KeyCode::Char('J') => controller::move_comment(st, 1, true),
        KeyCode::Char('K') => controller::move_comment(st, -1, true),
        KeyCode::Enter => {
            controller::begin_comment(st);
            let _ = tx; // begin_comment only opens the overlay
        }
        _ => {}
    }
}

fn handle_overlay_key(st: &mut State, tx: &mpsc::Sender<Job>, k: KeyEvent) {
    let m = k.modifiers;
    let shift = m.contains(KeyModifiers::SHIFT);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);

    if k.code == KeyCode::Esc {
        st.overlay = Overlay::None;
        return;
    }
    // Review — choices mode
    if matches!(&st.overlay, Overlay::Review { editing, .. } if !*editing) {
        if k.code == KeyCode::Enter {
            controller::confirm_review(st, tx);
            return;
        }
        if let Overlay::Review { editing, choice, .. } = &mut st.overlay {
            match k.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if *choice == 0 {
                        *editing = true;
                    } else {
                        *choice -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *choice = (*choice + 1).min(REVIEW_EVENTS.len() - 1);
                }
                _ => {}
            }
        }
        return;
    }
    // Editing mode (Comment / Edit / Review-editing)
    match k.code {
        KeyCode::Enter => {
            if shift {
                if let Some(ta) = overlay_ta(st) {
                    ta.newline();
                }
            } else {
                match &st.overlay {
                    Overlay::Comment { .. } => controller::confirm_comment(st, tx),
                    Overlay::Edit { .. } => controller::confirm_edit(st, tx),
                    Overlay::Review { .. } => {
                        if let Overlay::Review { editing, .. } = &mut st.overlay {
                            *editing = false;
                        }
                    }
                    Overlay::None => {}
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(ta) = overlay_ta(st) {
                if alt {
                    ta.delete_word();
                } else {
                    ta.backspace();
                }
            }
        }
        KeyCode::Char('w') if ctrl => {
            if let Some(ta) = overlay_ta(st) {
                ta.delete_word();
            }
        }
        KeyCode::Left => {
            if let Some(ta) = overlay_ta(st) {
                ta.left();
            }
        }
        KeyCode::Right => {
            if let Some(ta) = overlay_ta(st) {
                ta.right();
            }
        }
        KeyCode::Up => {
            if let Some(ta) = overlay_ta(st) {
                ta.up();
            }
        }
        KeyCode::Down => {
            if let Some(ta) = overlay_ta(st) {
                ta.down();
            }
        }
        KeyCode::Home => {
            if let Some(ta) = overlay_ta(st) {
                ta.home();
            }
        }
        KeyCode::End => {
            if let Some(ta) = overlay_ta(st) {
                ta.end();
            }
        }
        KeyCode::Char(c) if !ctrl && !alt => {
            if let Some(ta) = overlay_ta(st) {
                ta.insert(c);
            }
        }
        _ => {}
    }
}

// ---- small helpers ----

fn toggle_commit(st: &mut State) {
    if let Some(c) = st.commits.get(st.commit_idx) {
        let oid = c.oid.clone();
        if st.commit_selected.contains(&oid) {
            st.commit_selected.remove(&oid);
        } else {
            st.commit_selected.insert(oid);
        }
    }
}

fn toggle_all_commits(st: &mut State) {
    if st.commit_selected.len() == st.commits.len() {
        st.commit_selected.clear();
    } else {
        st.commit_selected = st.commits.iter().map(|c| c.oid.clone()).collect();
    }
}

fn jump_file(st: &mut State, direction: i64) {
    let mut i = st.file_idx as i64 + direction;
    while i >= 0 && (i as usize) < st.tree.len() {
        if matches!(st.tree[i as usize], TreeRow::File { .. }) {
            st.file_idx = i as usize;
            st.diff_scroll = 0;
            st.diff_hunk_idx = 0;
            return;
        }
        i += direction;
    }
}

fn open_file_or_dir(st: &mut State) {
    match st.tree.get(st.file_idx).cloned() {
        Some(TreeRow::File { .. }) => {
            st.focus = Focus::Diff;
            st.diff_scroll = 0;
            st.diff_hunk_idx = 0;
        }
        Some(TreeRow::Dir { path, .. }) => controller::toggle_collapse(st, &path),
        None => {}
    }
}

fn discard_pending(st: &mut State, tx: &mpsc::Sender<Job>) {
    if st.pending_idx >= st.pending.len() || st.busy.contains("pending") || st.active_pr.is_none() {
        return;
    }
    let removed = st.pending.remove(st.pending_idx);
    st.pending_idx = st.pending_idx.min(st.pending.len().saturating_sub(1));
    let pr = st.active_pr.clone().unwrap();
    let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
    controller::submit(
        st,
        tx,
        Job::DiscardPending { owner, name, number: pr.number, login, comment_id: removed.comment_id },
    );
    st.status = format!("Discarding comment on {}:{}…", removed.path, removed.line);
}

fn handle_mouse(st: &mut State, m: MouseEvent, area: Rect) {
    let (rects, _, _) = ui::compute_layout(area);
    let (col, row) = (m.column, m.row);
    let hit = |r: Rect| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height;
    let pane = if hit(rects.prs) {
        Some(Focus::Prs)
    } else if hit(rects.commits) {
        Some(Focus::Commits)
    } else if hit(rects.files) {
        Some(Focus::Files)
    } else if hit(rects.pending) {
        Some(Focus::Pending)
    } else if hit(rects.right) {
        Some(Focus::Diff)
    } else {
        None
    };
    let Some(pane) = pane else { return };
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => st.focus = pane,
        MouseEventKind::ScrollDown => scroll_pane(st, pane, 3),
        MouseEventKind::ScrollUp => scroll_pane(st, pane, -3),
        _ => {}
    }
}

fn scroll_pane(st: &mut State, pane: Focus, delta: i64) {
    let bump = |v: &mut usize, d: i64| *v = (*v as i64 + d).max(0) as usize;
    match pane {
        Focus::Prs => bump(&mut st.pr_offset, delta),
        Focus::Commits => bump(&mut st.commit_offset, delta),
        Focus::Files => bump(&mut st.file_offset, delta),
        Focus::Pending => bump(&mut st.pending_offset, delta),
        Focus::Diff => {
            if st.focus == Focus::Prs {
                bump(&mut st.details_scroll, delta);
            } else {
                nav::scroll_diff(st, delta);
            }
        }
    }
}
