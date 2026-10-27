//! Curses/ratatui bootstrap, the event loop and key/mouse dispatch.

use std::io::stdout;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture, Event,
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton,
    MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;

use crate::models::{Category, CodePane, CodeSelect, CommitKind, Focus, Overlay, State, TextSelect, TreeRow, SUBMIT_CHOICES};
use crate::navigation as nav;
use crate::textbuffer::{self, TextArea};
use crate::worker::{Job, Msg};
use crate::{api, controller, editor, theme, ui, worker};

pub fn run(open_file: Option<String>, open_commit: Option<Vec<String>>, open_edits: bool) -> Result<()> {
    // Ask the terminal for its background before anything else reads the tty.
    theme::init_appearance(match theme::forced_appearance() {
        Some(_) => None,
        None => crate::term::background(),
    });
    let mut terminal = ratatui::init();
    // Best-effort: distinct Shift/Ctrl+Enter, Alt+Backspace on supporting terminals.
    let _ = execute!(
        stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        EnableMouseCapture,
        EnableFocusChange,
    );
    let result = event_loop(&mut terminal, open_file, open_commit, open_edits);
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags, DisableMouseCapture, DisableFocusChange);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    open_file: Option<String>,
    open_commit: Option<Vec<String>>,
    open_edits: bool,
) -> Result<()> {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
    let paint_tx = msg_tx.clone();
    thread::spawn(move || worker::worker_loop(job_rx, msg_tx));

    // Every parse runs on a thread of its own; drawing only reads what is done.
    let mut st = State {
        highlight: crate::syntax::Highlighter::threaded(paint_tx),
        ..Default::default()
    };
    match api::detect_repo() {
        Ok((o, n)) => {
            st.repo_owner = o;
            st.repo_name = n;
        }
        Err(e) => st.status = format!("detect_repo: {e}"),
    }
    st.repo_root = api::get_repo_root();
    st.viewer = api::get_viewer_login();
    let mut prev_view = api::load_view_modes();
    (st.side_by_side, st.ignore_whitespace) = prev_view;

    // Listen for another tool asking us to show a file. Held for the whole run:
    // dropping it removes the socket.
    let (ipc_tx, ipc_rx) = mpsc::channel::<crate::ipc::Request>();
    let _ipc = crate::ipc::listen(&st.repo_root, ipc_tx);
    if let Some(file) = open_file {
        st.pending_open_file = Some(crate::ipc::relative_to(&st.repo_root, &file));
    }
    st.pending_open_commit = open_commit;
    st.pending_open_edits = open_edits;
    if !st.repo_owner.is_empty() {
        let repo_root = st.repo_root.clone();
        controller::submit(&mut st, &job_tx, Job::LoadPrs { repo_root });
    }

    let mut prev_pr = usize::MAX;
    let mut prev_focus: Option<Focus> = None;
    let mut theme_watch = theme::Watch::default();
    let mut graceful = false;
    loop {
        while let Ok(msg) = msg_rx.try_recv() {
            controller::apply_msg(&mut st, msg, &job_tx);
        }
        while let Ok(req) = ipc_rx.try_recv() {
            match req {
                crate::ipc::Request::Open(file) => {
                    st.pending_open_file =
                        Some(crate::ipc::relative_to(&st.repo_root, &file));
                    // The request came from another window, so bring ours forward.
                    editor::focus_self();
                }
                crate::ipc::Request::Commit(sha) => {
                    st.pending_open_commit = Some(sha);
                    editor::focus_self();
                }
                crate::ipc::Request::Focus => editor::focus_self(),
                crate::ipc::Request::Edits => {
                    st.pending_open_edits = true;
                    editor::focus_self();
                }
                // Treated as a clean quit, which is what runs the cleanup that
                // closes the windows this session opened.
                crate::ipc::Request::Quit => st.should_quit = true,
            }
        }
        controller::try_open_pending_file(&mut st, &job_tx);
        controller::try_open_pending_commit(&mut st, &job_tx);
        controller::try_open_pending_edits(&mut st, &job_tx);
        if st.focus == Focus::Prs {
            if st.pr_idx != prev_pr || prev_focus != Some(Focus::Prs) {
                st.details_scroll = 0;
            }
            controller::maybe_load_details(&mut st, &job_tx);
        }
        // Entering the Pending-edits pane refreshes the worktree change list.
        if st.focus == Focus::Edits && prev_focus != Some(Focus::Edits) {
            controller::reload_edits(&mut st, &job_tx);
        }
        // [3] and [4] each show their own file, so a switch between them, or
        // a reload that moved the [4] cursor, starts on the first stop.
        let shown = nav::diff_path(&st);
        if shown != st.diff_shown_path {
            st.diff_shown_path = shown;
            reset_diff_cursor(&mut st);
        }
        prev_pr = st.pr_idx;
        prev_focus = Some(st.focus);
        if (st.side_by_side, st.ignore_whitespace) != prev_view {
            prev_view = (st.side_by_side, st.ignore_whitespace);
            api::save_view_modes(prev_view);
        }

        // Detect a closed per-worktree editor; ungroup once the last one is gone.
        poll_worktree_editors(&mut st);
        // The next draw paints the new theme; the highlight cache holds roles,
        // so nothing is parsed again.
        theme_watch.poll();

        // A draw/read error means the terminal went away (window closed) — leave
        // the loop so cleanup still runs.
        if terminal.draw(|f| ui::render(f, &mut st)).is_err() {
            break;
        }
        if st.should_quit {
            graceful = true;
            break;
        }

        let area = size_rect(terminal);
        match event::poll(Duration::from_millis(80)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => handle_key(&mut st, &job_tx, k, area),
                Ok(Event::Mouse(m)) => handle_mouse(&mut st, m, area),
                // Returning to the TUI window re-scans the worktree for edits.
                Ok(Event::FocusGained) => controller::reload_edits(&mut st, &job_tx),
                Ok(_) => {}
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    }
    cleanup_editors(&mut st, graceful);
    let _ = job_tx.send(Job::Quit);
    Ok(())
}

/// Remove closed per-worktree editors from tracking; when the last one is gone,
/// dissolve the group we created.
fn poll_worktree_editors(st: &mut State) {
    if st.worktree_editors.is_empty() {
        return;
    }
    let mut closed = Vec::new();
    for (sock, seen_alive) in st.worktree_editors.iter_mut() {
        if editor::socket_exists(sock) {
            *seen_alive = true; // it has come up
        } else if *seen_alive {
            closed.push(sock.clone()); // was up, now gone
        }
    }
    for sock in &closed {
        st.worktree_editors.remove(sock);
    }
    if !closed.is_empty() && st.worktree_editors.is_empty() && st.entered_group {
        editor::ungroup_active();
        st.entered_group = false;
    }
}

/// Close the per-worktree Neovim windows we launched and leave the group (the
/// TUI is still focused here). Used when switching PRs so dead editors don't
/// pile up in the group.
fn close_worktree_editors(st: &mut State) {
    if st.entered_group {
        editor::ungroup_active();
        st.entered_group = false;
    }
    for sock in st.worktree_editors.keys() {
        editor::close_worktree_editor(sock);
    }
    st.worktree_editors.clear();
}

/// On exit: close the Neovim windows we launched, and (on a clean quit, while the
/// TUI window is still focused) leave the Hyprland group.
fn cleanup_editors(st: &mut State, graceful: bool) {
    for sock in st.worktree_editors.keys() {
        editor::close_worktree_editor(sock);
    }
    st.worktree_editors.clear();
    // The Claude window is ours too; leaving it behind strands an agent nobody
    // is watching.
    editor::close_review_claude();

    // Last, and only on a clean quit: ungrouping needs our window to still be
    // focused, and a group we did not create is not ours to dissolve.
    if graceful && st.entered_group {
        editor::ungroup_active();
        st.entered_group = false;
    }
}

fn size_rect(terminal: &ratatui::DefaultTerminal) -> Rect {
    terminal
        .size()
        .map(|s| Rect { x: 0, y: 0, width: s.width, height: s.height })
        .unwrap_or(Rect { x: 0, y: 0, width: 80, height: 24 })
}

fn overlay_ta(st: &mut State) -> Option<&mut TextArea> {
    match &mut st.overlay {
        Overlay::Comment { ta, .. }
        | Overlay::Edit { ta, .. }
        | Overlay::Review { ta, .. }
        | Overlay::CommitMsg { ta, .. }
        | Overlay::Ask { ta } => Some(ta),
        Overlay::None | Overlay::Confirm { .. } | Overlay::Hooks { .. } | Overlay::Archive { .. } => None,
    }
}

fn handle_key(st: &mut State, tx: &mpsc::Sender<Job>, k: KeyEvent, area: Rect) {
    if !matches!(st.overlay, Overlay::None) {
        handle_overlay_key(st, tx, k);
        return;
    }
    if st.code_select.is_some() {
        if is_copy_key(&k) {
            copy_code_selection(st);
            return;
        }
        st.code_select = None;
    }
    if st.comment_mode {
        handle_comment_mode(st, tx, k);
        return;
    }
    match k.code {
        KeyCode::Char('q') => st.should_quit = true,
        KeyCode::Char(c @ '0'..='5') => {
            if let Some(f) = Focus::from_digit(c) {
                st.focus = f;
            }
        }
        KeyCode::Char('J') => match st.focus {
            Focus::Prs => st.details_scroll += 1,
            Focus::Files => set_file_idx(st, st.file_idx + 1),
            Focus::Edits => set_edit_idx(st, st.edit_idx + 1),
            _ => nav::scroll_diff(st, 1),
        },
        KeyCode::Char('K') => match st.focus {
            Focus::Prs => st.details_scroll = st.details_scroll.saturating_sub(1),
            Focus::Files => set_file_idx(st, st.file_idx.saturating_sub(1)),
            Focus::Edits => set_edit_idx(st, st.edit_idx.saturating_sub(1)),
            _ => nav::scroll_diff(st, -1),
        },
        KeyCode::Tab => st.focus = st.focus.next(),
        KeyCode::BackTab => st.focus = st.focus.prev(),
        KeyCode::Char('r') => refresh(st, tx),
        KeyCode::Char('g') => open_pr_browser(st),
        KeyCode::Char('D') => controller::deep_review(st),
        _ => handle_pane_key(st, tx, k, area),
    }
}

fn refresh(st: &mut State, tx: &mpsc::Sender<Job>) {
    if !st.busy.contains("prs") {
        let repo_root = st.repo_root.clone();
        controller::submit(st, tx, Job::LoadPrs { repo_root });
    }
    if let Some(pr) = st.active_pr.clone() {
        if st.local_mode {
            if !st.busy.contains("active") {
                let (owner, name, login) = (st.repo_owner.clone(), st.repo_name.clone(), st.viewer.clone());
                let wt = st.active_worktree.clone();
                let ignore_ws = st.ignore_whitespace;
                controller::submit(st, tx, Job::LoadActive { wt, owner, name, login, number: Some(pr.number), local: true, ignore_ws });
            }
        } else if !st.busy.contains("worktree") && !st.busy.contains("active") {
            let (repo_root, owner, name) = (st.repo_root.clone(), st.repo_owner.clone(), st.repo_name.clone());
            controller::submit(st, tx, Job::OpenPr { repo_root, owner, name, number: pr.number, head: pr.head.clone() });
        }
    }
    if st.focus == Focus::Prs && !st.prs.is_empty() {
        let n = st.prs[st.pr_idx].number;
        st.pr_details.remove(&n);
        controller::maybe_load_details(st, tx);
    }
    controller::reload_edits(st, tx);
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
                    // Switching PRs closes any worktree editor left in the group.
                    close_worktree_editors(st);
                    if pr.category == Category::CheckedOut {
                        controller::begin_open_local_pr(st, tx, pr);
                    } else {
                        controller::begin_open_pr(st, tx, pr);
                    }
                }
            }
            KeyCode::Char('C') => checkout_local(st, tx),
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
            KeyCode::Char('j') if alt => pending_jump_file(st, 1),
            KeyCode::Char('k') if alt => pending_jump_file(st, -1),
            KeyCode::Down | KeyCode::Char('j') => pending_move(st, 1),
            KeyCode::Up | KeyCode::Char('k') => pending_move(st, -1),
            KeyCode::Char('z') => pending_jump_file(st, 1),
            KeyCode::Enter => controller::begin_review(st),
            KeyCode::Char('e') => {
                if !st.busy.contains("pending") {
                    controller::begin_edit_pending(st);
                }
            }
            KeyCode::Char('d') => controller::discard_selected_comment(st, tx),
            KeyCode::Char('H') => controller::open_archive(st),
            _ => {}
        },
        Focus::Files => match k.code {
            KeyCode::Char('j') if alt => jump_file(st, 1),
            KeyCode::Char('k') if alt => jump_file(st, -1),
            KeyCode::Down | KeyCode::Char('j') => step_file_stop(st, 1),
            KeyCode::Up | KeyCode::Char('k') => step_file_stop(st, -1),
            KeyCode::Home => {
                set_file_idx(st, 0);
                land_on_file(st, 1);
            }
            KeyCode::Char('G') => {
                set_file_idx(st, st.tree.len().saturating_sub(1));
                land_on_file(st, -1);
            }
            KeyCode::PageDown => nav::scroll_diff(st, page as i64),
            KeyCode::PageUp => nav::scroll_diff(st, -(page as i64)),
            KeyCode::Enter => match st.tree.get(st.file_idx).cloned() {
                Some(TreeRow::Dir { path, .. }) => controller::toggle_collapse(st, &path),
                _ => controller::enter_focused_stop(st),
            },
            KeyCode::Char(' ') => controller::mark_viewed(st, tx),
            KeyCode::Char('z') => controller::fold_viewed(st, 1),
            KeyCode::Char('Z') => controller::fold_viewed(st, -1),
            KeyCode::Char('U') => {
                st.collapsed_dirs.clear();
                crate::tree::rebuild(st);
                st.status = "Unfolded all folders.".into();
            }
            KeyCode::Char('d') => {
                controller::discard_focused_comment(st, tx);
            }
            KeyCode::Char('o') => controller::open_focused_thread(st),
            KeyCode::Char('e') if selected_is_file(st) => edit_focused_stop(st),
            KeyCode::Char('e') => editor::open_current_in_editor(st, true),
            KeyCode::Char('c') if selected_is_file(st) => controller::enter_comment_mode(st),
            KeyCode::Char('a') if selected_is_file(st) => controller::begin_ask(st),
            KeyCode::Char('s') => controller::toggle_side_by_side(st),
            KeyCode::Char('w') => controller::toggle_whitespace(st, tx),
            _ => {}
        },
        Focus::Edits => match k.code {
            KeyCode::Char('j') if alt => jump_edit_file(st, 1),
            KeyCode::Char('k') if alt => jump_edit_file(st, -1),
            KeyCode::Down | KeyCode::Char('j') => step_edit_stop(st, 1),
            KeyCode::Up | KeyCode::Char('k') => step_edit_stop(st, -1),
            KeyCode::Home => {
                set_edit_idx(st, 0);
                land_on_edit_file(st, 1);
            }
            KeyCode::Char('G') => {
                set_edit_idx(st, st.edit_tree.len().saturating_sub(1));
                land_on_edit_file(st, -1);
            }
            KeyCode::Char('z') => controller::fold_staged(st, 1),
            KeyCode::Char('Z') => controller::fold_staged(st, -1),
            KeyCode::PageDown => nav::scroll_diff(st, page as i64),
            KeyCode::PageUp => nav::scroll_diff(st, -(page as i64)),
            KeyCode::Enter => controller::fold_edit_dir(st),
            KeyCode::Char(' ') => controller::toggle_stage(st, tx),
            KeyCode::Char('c') => controller::begin_commit_edits(st, CommitKind::New),
            KeyCode::Char('w') => controller::begin_commit_edits(st, CommitKind::NoVerify),
            KeyCode::Char('A') => controller::begin_commit_edits(st, CommitKind::Amend),
            KeyCode::Char('P') => controller::push_edits(st, tx),
            KeyCode::Char('d') => controller::begin_discard_hunk(st),
            KeyCode::Char('X') => controller::begin_discard_edit(st),
            KeyCode::Char('U') => {
                st.edit_collapsed.clear();
                crate::tree::rebuild_edits(st);
                st.status = "Unfolded all folders.".into();
            }
            KeyCode::Char('e') => editor::open_current_edit_in_editor(st),
            KeyCode::Char('s') => controller::toggle_side_by_side(st),
            _ => {}
        },
    }
}

fn handle_comment_mode(st: &mut State, tx: &mpsc::Sender<Job>, k: KeyEvent) {
    match k.code {
        KeyCode::Esc => {
            st.comment_mode = false;
            // Drafts are per-file and already saved; leaving the picker keeps them.
            st.status = "Comment cancelled.".into();
        }
        KeyCode::Down | KeyCode::Char('j') => controller::move_comment(st, 1, false),
        KeyCode::Up | KeyCode::Char('k') => controller::move_comment(st, -1, false),
        KeyCode::Char('J') => controller::move_comment(st, 1, true),
        KeyCode::Char('K') => controller::move_comment(st, -1, true),
        KeyCode::Enter => {
            controller::begin_comment_or_edit(st);
            let _ = tx; // only opens an overlay
        }
        _ => {}
    }
}

fn handle_overlay_key(st: &mut State, tx: &mpsc::Sender<Job>, k: KeyEvent) {
    let m = k.modifiers;
    let shift = m.contains(KeyModifiers::SHIFT);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);

    if is_copy_key(&k) && st.text_select.is_some() {
        copy_selection(st);
        return;
    }
    st.text_select = None;

    // Hook output: nothing to type into. While they run, Esc puts the window
    // away without cancelling them — a hook that hangs must not take the
    // keyboard with it — and everything else is ignored.
    if let Overlay::Hooks { failed, lines, scroll, .. } = &mut st.overlay {
        if !*failed {
            if k.code == KeyCode::Esc {
                st.overlay = Overlay::None;
                st.status = "Hooks still running — the result lands in the status bar.".into();
            }
            return;
        }
        match k.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => st.overlay = Overlay::None,
            KeyCode::Down | KeyCode::Char('j') => {
                *scroll = (*scroll + 1).min(lines.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
            KeyCode::PageDown => *scroll = (*scroll + 10).min(lines.len().saturating_sub(1)),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::Char('g') => *scroll = 0,
            KeyCode::Char('G') => *scroll = lines.len().saturating_sub(1),
            _ => {}
        }
        return;
    }
    if k.code == KeyCode::Esc {
        // Closing a new-comment editor returns to the line picker (keeping the
        // draft + selection); other overlays just close.
        if matches!(st.overlay, Overlay::Comment { .. }) {
            controller::comment_to_picker(st);
        } else {
            st.overlay = Overlay::None;
        }
        return;
    }
    // Archive: a list to walk, nothing to type into.
    if matches!(st.overlay, Overlay::Archive { .. }) {
        match k.code {
            KeyCode::Enter => controller::restore_archive_batch(st, tx),
            KeyCode::Down | KeyCode::Char('j') => controller::archive_move(st, 1),
            KeyCode::Up | KeyCode::Char('k') => controller::archive_move(st, -1),
            KeyCode::PageDown | KeyCode::Char('J') => controller::archive_scroll(st, 10),
            KeyCode::PageUp | KeyCode::Char('K') => controller::archive_scroll(st, -10),
            KeyCode::Char('q') => st.overlay = Overlay::None,
            _ => {}
        }
        return;
    }
    // Yes/no confirmation.
    if matches!(st.overlay, Overlay::Confirm { .. }) {
        match k.code {
            KeyCode::Char('y') | KeyCode::Enter => controller::confirm_action(st, tx),
            _ => st.overlay = Overlay::None,
        }
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
                    *choice = (*choice + 1).min(SUBMIT_CHOICES.len() - 1);
                }
                _ => {}
            }
        }
        return;
    }
    // Editing mode (Comment / Edit / Review-editing)
    match k.code {
        KeyCode::Enter => {
            // Shift+Enter needs the kitty keyboard protocol (flaky across
            // terminals); Alt+Enter is delivered reliably via the meta-escape.
            if shift || alt {
                if let Some(ta) = overlay_ta(st) {
                    ta.newline();
                }
            } else {
                match &st.overlay {
                    Overlay::Comment { .. } => controller::confirm_comment(st, tx),
                    Overlay::Edit { .. } => controller::confirm_edit(st, tx),
                    Overlay::CommitMsg { .. } => controller::confirm_commit_edits(st, tx),
                    Overlay::Ask { .. } => controller::confirm_ask(st),
                    Overlay::Review { .. } => {
                        if let Overlay::Review { editing, .. } = &mut st.overlay {
                            *editing = false;
                        }
                    }
                    Overlay::None | Overlay::Confirm { .. } | Overlay::Hooks { .. } | Overlay::Archive { .. } => {}
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(ta) = overlay_ta(st) {
                if alt || ctrl {
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
        KeyCode::Char('s') if ctrl => controller::insert_suggestion(st),
        KeyCode::Char('d') if ctrl => {
            if matches!(st.overlay, Overlay::Edit { .. }) {
                controller::delete_editing_comment(st, tx);
            }
        }
        KeyCode::Left => {
            if let Some(ta) = overlay_ta(st) {
                if ctrl {
                    ta.word_left();
                } else {
                    ta.left();
                }
            }
        }
        KeyCode::Right => {
            if let Some(ta) = overlay_ta(st) {
                if ctrl {
                    ta.word_right();
                } else {
                    ta.right();
                }
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

fn reset_diff_cursor(st: &mut State) {
    st.diff_scroll = 0;
    st.diff_hunk_idx = 0;
    st.diff_stop_idx = 0;
    st.diff_reveal_pending = true;
}

fn set_file_idx(st: &mut State, idx: usize) {
    st.file_idx = idx.min(st.tree.len().saturating_sub(1));
    reset_diff_cursor(st);
}

fn set_edit_idx(st: &mut State, idx: usize) {
    st.edit_idx = idx.min(st.edit_tree.len().saturating_sub(1));
    reset_diff_cursor(st);
}

/// Move to the next/previous file row in the Files tree, skipping folders.
fn jump_file(st: &mut State, direction: i64) {
    if let Some(i) = next_file_row(&st.tree, st.file_idx, direction) {
        set_file_idx(st, i);
    }
}

/// Move to the next/previous file row in the Pending-edits tree, skipping folders.
fn jump_edit_file(st: &mut State, direction: i64) {
    if let Some(i) = next_file_row(&st.edit_tree, st.edit_idx, direction) {
        set_edit_idx(st, i);
    }
}

fn next_file_row(tree: &[TreeRow], from: usize, direction: i64) -> Option<usize> {
    let mut i = from as i64 + direction;
    while i >= 0 && (i as usize) < tree.len() {
        if matches!(tree[i as usize], TreeRow::File { .. }) {
            return Some(i as usize);
        }
        i += direction;
    }
    None
}

/// A folder row has no diff to walk, so the cursor moves on to a file.
fn land_on_file(st: &mut State, direction: i64) {
    if !selected_is_file(st) {
        jump_file(st, direction);
    }
}

fn land_on_edit_file(st: &mut State, direction: i64) {
    if !matches!(st.edit_tree.get(st.edit_idx), Some(TreeRow::File { .. })) {
        jump_edit_file(st, direction);
    }
}

/// `j` / `k` in the Files pane: walk the stops of the selected file's diff.
fn step_file_stop(st: &mut State, direction: i64) {
    if selected_is_file(st) {
        nav::step_stop(st, direction);
    } else {
        jump_file(st, direction);
    }
}

fn step_edit_stop(st: &mut State, direction: i64) {
    if matches!(st.edit_tree.get(st.edit_idx), Some(TreeRow::File { .. })) {
        nav::step_stop(st, direction);
    } else {
        jump_edit_file(st, direction);
    }
}

/// Move the pending-comment selection by one, in display (tree) order.
fn pending_move(st: &mut State, direction: i64) {
    let order = nav::pending_order(st);
    if order.is_empty() {
        return;
    }
    let pos = order.iter().position(|&i| i == st.pending_idx).unwrap_or(0);
    let np = (pos as i64 + direction).clamp(0, order.len() as i64 - 1) as usize;
    st.pending_idx = order[np];
}

/// Jump the pending-comment selection to the first comment of the next/previous
/// file group.
fn pending_jump_file(st: &mut State, direction: i64) {
    let order = nav::pending_order(st);
    if order.is_empty() {
        return;
    }
    let pos = order.iter().position(|&i| i == st.pending_idx).unwrap_or(0);
    let cur = st.pending[order[pos]].path.clone();
    if direction > 0 {
        for j in pos + 1..order.len() {
            if st.pending[order[j]].path != cur {
                st.pending_idx = order[j];
                return;
            }
        }
    } else {
        // Back up to the start of the current group, then to the start of the one before.
        let mut start = pos;
        while start > 0 && st.pending[order[start - 1]].path == cur {
            start -= 1;
        }
        if start == 0 {
            st.pending_idx = order[0];
            return;
        }
        let prev = st.pending[order[start - 1]].path.clone();
        let mut ps = start - 1;
        while ps > 0 && st.pending[order[ps - 1]].path == prev {
            ps -= 1;
        }
        st.pending_idx = order[ps];
    }
}

fn edit_focused_stop(st: &mut State) {
    if !controller::edit_focused_comment(st) {
        editor::open_current_in_editor(st, false);
    }
}

fn selected_is_file(st: &State) -> bool {
    matches!(st.tree.get(st.file_idx), Some(TreeRow::File { .. }))
}

fn open_pr_browser(st: &mut State) {
    let number = match (&st.active_pr, st.prs.get(st.pr_idx)) {
        (Some(active), _) if st.focus != Focus::Prs => active.number,
        (_, Some(selected)) => selected.number,
        _ => return,
    };
    editor::open_pr_in_browser(&st.repo_owner, &st.repo_name, number);
    st.status = format!("Opening #{number} in browser…");
}

fn checkout_local(st: &mut State, tx: &mpsc::Sender<Job>) {
    if st.prs.is_empty() || st.busy.contains("checkout") {
        return;
    }
    let Ok(home) = std::env::var("HOME") else {
        st.status = "HOME not set.".into();
        return;
    };
    let number = st.prs[st.pr_idx].number;
    let (owner, name) = (st.repo_owner.clone(), st.repo_name.clone());
    let dir = format!("{home}/Projects/{name}");
    st.status = format!("Checking out #{number} in {dir}…");
    controller::submit(st, tx, Job::CheckoutLocal { dir, owner, name, number });
}


fn handle_mouse(st: &mut State, m: MouseEvent, area: Rect) {
    if overlay_ta(st).is_some() {
        handle_modal_mouse(st, m);
        return;
    }
    if handle_code_mouse(st, m) {
        return;
    }
    let (rects, _, _) = ui::compute_layout(area, st);
    let (col, row) = (m.column, m.row);
    let hit = |r: Rect| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height;
    let pane = if hit(rects.prs) {
        Some(Focus::Prs)
    } else if hit(rects.commits) {
        Some(Focus::Commits)
    } else if hit(rects.files) {
        Some(Focus::Files)
    } else if hit(rects.edits) {
        Some(Focus::Edits)
    } else if hit(rects.pending) {
        Some(Focus::Pending)
    } else {
        None
    };
    let delta = match m.kind {
        MouseEventKind::ScrollDown => 3,
        MouseEventKind::ScrollUp => -3,
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(pane) = pane {
                st.focus = pane;
            }
            return;
        }
        _ => return,
    };
    match pane {
        Some(pane) => scroll_pane(st, pane, delta),
        None if hit(rects.right) => scroll_right(st, delta),
        None => {}
    }
}

/// Drag over a modal's text to select it, like a terminal does. The release
/// copies the selection; the highlight stays until the next key or click.
fn handle_modal_mouse(st: &mut State, m: MouseEvent) {
    let Some(text) = st.modal_text else { return };
    let inside = m.column >= text.x
        && m.column < text.x + text.width
        && m.row >= text.y
        && m.row < text.y + text.height;
    let vrow = text.first + m.row.saturating_sub(text.y).min(text.height.saturating_sub(1)) as usize;
    let vcol = m.column.saturating_sub(text.x).min(text.width.saturating_sub(1)) as usize;
    let width = text.width.max(1) as usize;
    let Some(ta) = overlay_ta(st) else { return };
    let pos = textbuffer::position_at(ta, width, vrow, vcol);
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            st.text_select = inside.then_some(TextSelect { anchor: pos, head: pos });
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(sel) = &mut st.text_select {
                sel.head = pos;
            }
        }
        MouseEventKind::Up(MouseButton::Left) => copy_selection(st),
        _ => {}
    }
}

/// Drag over a diff code column to select what it shows. The selection stays
/// in the column where the drag starts, so a side-by-side drag copies one side.
/// Returns whether the event belongs to such a drag.
fn handle_code_mouse(st: &mut State, m: MouseEvent) -> bool {
    let at = |pane: &CodePane| {
        let r = pane.rect;
        let row = m.row.clamp(r.y, r.bottom().saturating_sub(1)) - r.y;
        let col = m.column.clamp(r.x, r.right().saturating_sub(1)) - r.x;
        (row, col)
    };
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let hit = st.code_panes.iter().position(|p| p.rect.contains((m.column, m.row).into()));
            st.code_select = hit.map(|i| {
                let pos = at(&st.code_panes[i]);
                CodeSelect { pane: i, anchor: pos, head: pos }
            });
            hit.is_some()
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let Some(sel) = st.code_select else { return false };
            let Some(pane) = st.code_panes.get(sel.pane) else { return false };
            let head = at(pane);
            st.code_select = Some(CodeSelect { head, ..sel });
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            copy_code_selection(st);
            st.code_select.is_some()
        }
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            st.code_select = None;
            false
        }
        _ => false,
    }
}

fn copy_code_selection(st: &mut State) {
    let Some(sel) = st.code_select.filter(|s| s.anchor != s.head) else { return };
    let Some(pane) = st.code_panes.get(sel.pane) else { return };
    let text = ui::selected_cells(&pane.rows, sel.anchor, sel.head);
    editor::copy_to_clipboard(&text);
    st.status = format!("Copied {} characters.", text.chars().count());
}

fn is_copy_key(k: &KeyEvent) -> bool {
    match k.code {
        KeyCode::Insert => k.modifiers.contains(KeyModifiers::CONTROL),
        KeyCode::Char('c' | 'C') => k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER),
        _ => false,
    }
}

fn copy_selection(st: &mut State) {
    let Some(sel) = st.text_select.filter(|s| s.anchor != s.head) else { return };
    let Some(ta) = overlay_ta(st) else { return };
    let text = textbuffer::selected_text(ta, sel.anchor, sel.head);
    editor::copy_to_clipboard(&text);
    st.status = format!("Copied {} characters.", text.chars().count());
}

fn scroll_pane(st: &mut State, pane: Focus, delta: i64) {
    let bump = |v: &mut usize, d: i64| *v = (*v as i64 + d).max(0) as usize;
    match pane {
        Focus::Prs => bump(&mut st.pr_offset, delta),
        Focus::Commits => bump(&mut st.commit_offset, delta),
        Focus::Files => bump(&mut st.file_offset, delta),
        Focus::Edits => bump(&mut st.edit_offset, delta),
        Focus::Pending => bump(&mut st.pending_offset, delta),
    }
}

fn scroll_right(st: &mut State, delta: i64) {
    if st.focus == Focus::Prs {
        st.details_scroll = (st.details_scroll as i64 + delta).max(0) as usize;
    } else {
        nav::scroll_diff(st, delta);
    }
}
