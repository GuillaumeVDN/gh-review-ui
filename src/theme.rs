//! Colors and diff/highlight styling (ratatui `Style`s).

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};

use crate::markdown::Kind;

/// Whether the terminal paints on a dark or a light ground.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Appearance {
    Dark,
    Light,
}

/// The diff backgrounds, one set per terminal appearance.
///
/// Added and deleted lines carry the diff meaning in the background, so the
/// code on them keeps its syntax colors. The focused change block takes the
/// brighter pair of the same two hues.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub add_bg: Color,
    pub del_bg: Color,
    pub add_bg_current: Color,
    pub del_bg_current: Color,
}

const DARK: Palette = Palette {
    add_bg: Color::Indexed(22),
    del_bg: Color::Indexed(52),
    add_bg_current: Color::Indexed(28),
    del_bg_current: Color::Indexed(88),
};

const LIGHT: Palette = Palette {
    add_bg: Color::Indexed(194),
    del_bg: Color::Indexed(224),
    add_bg_current: Color::Indexed(157),
    del_bg_current: Color::Indexed(217),
};

/// The terminal appearance, from `GH_REVIEW_UI_THEME` or `COLORFGBG`.
///
/// `COLORFGBG` holds `fg;bg`, and some terminals put a third field between the
/// two. The background index is the last field.
pub fn detect_appearance(forced: Option<&str>, colorfgbg: Option<&str>) -> Appearance {
    match forced.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("light") => return Appearance::Light,
        Some("dark") => return Appearance::Dark,
        _ => {}
    }
    let bg = colorfgbg
        .and_then(|v| v.rsplit(';').next())
        .and_then(|v| v.trim().parse::<u8>().ok());
    match bg {
        Some(0..=6) | Some(8) => Appearance::Dark,
        Some(7) | Some(15) => Appearance::Light,
        _ => Appearance::Dark,
    }
}

pub fn appearance() -> Appearance {
    static FOUND: OnceLock<Appearance> = OnceLock::new();
    *FOUND.get_or_init(|| {
        detect_appearance(
            std::env::var("GH_REVIEW_UI_THEME").ok().as_deref(),
            std::env::var("COLORFGBG").ok().as_deref(),
        )
    })
}

pub fn palette() -> &'static Palette {
    match appearance() {
        Appearance::Dark => &DARK,
        Appearance::Light => &LIGHT,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffKind {
    Add,
    Del,
    Hunk,
    Meta,
    Context,
}

pub fn classify_diff_line(line: &str) -> DiffKind {
    if line.starts_with('+') && !line.starts_with("+++") {
        DiffKind::Add
    } else if line.starts_with('-') && !line.starts_with("---") {
        DiffKind::Del
    } else if line.starts_with("@@") {
        DiffKind::Hunk
    } else if line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("+++")
        || line.starts_with("---")
    {
        DiffKind::Meta
    } else {
        DiffKind::Context
    }
}

/// The background a diff row sits on: a tint for a changed line, nothing for
/// context and headers. The focused change block takes the brighter tint.
pub fn diff_row_style(kind: DiffKind, current: bool) -> Style {
    let p = palette();
    match (kind, current) {
        (DiffKind::Add, false) => Style::default().bg(p.add_bg),
        (DiffKind::Add, true) => Style::default().bg(p.add_bg_current),
        (DiffKind::Del, false) => Style::default().bg(p.del_bg),
        (DiffKind::Del, true) => Style::default().bg(p.del_bg_current),
        _ => Style::default(),
    }
}

/// The `+` / `-` of a changed line, in the gutter.
pub fn diff_marker_style(kind: DiffKind, current: bool) -> Style {
    let fg = match kind {
        DiffKind::Add => Color::Green,
        DiffKind::Del => Color::Red,
        _ => return diff_row_style(kind, current),
    };
    diff_row_style(kind, current).fg(fg).add_modifier(Modifier::BOLD)
}

/// The line numbers of a diff row: the change color for a changed line, dim
/// for everything else.
pub fn diff_number_style(kind: DiffKind, current: bool) -> Style {
    match kind {
        DiffKind::Add => diff_row_style(kind, current).fg(Color::Green),
        DiffKind::Del => diff_row_style(kind, current).fg(Color::Red),
        _ => dim(),
    }
}

/// Style for a whole diff line, code included. The panes that color the code
/// itself use [`diff_row_style`] and the syntax colors instead.
pub fn diff_line_style(line: &str, current: bool) -> Style {
    let k = classify_diff_line(line);
    match k {
        DiffKind::Add | DiffKind::Del => diff_marker_style(k, current),
        DiffKind::Hunk => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        DiffKind::Meta => Style::default().add_modifier(Modifier::BOLD),
        DiffKind::Context => Style::default(),
    }
}

pub fn hunk_marker() -> Style {
    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
}

pub fn comment_marker() -> Style {
    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
}

/// Inline pending-comment body shown under a diff line.
pub fn comment_inline() -> Style {
    Style::default().fg(Color::Cyan)
}

// Local (uncommitted worktree) edits overlaid on the PR diff, in orange.
const LOCAL: Color = Color::Indexed(208); // orange

pub fn local_add() -> Style {
    Style::default().fg(LOCAL)
}
pub fn local_del() -> Style {
    Style::default().fg(LOCAL).add_modifier(Modifier::CROSSED_OUT)
}
pub fn local_marker() -> Style {
    Style::default().fg(LOCAL).add_modifier(Modifier::BOLD)
}

/// A commit that is on no remote, in the same orange as the uncommitted edits:
/// both are local work, still yours to rewrite, and neither is on the PR.
pub fn unpushed_commit() -> Style {
    Style::default().fg(LOCAL)
}

/// Row style for a pending-edit entry, by change kind.
pub fn edit_kind_style(kind: crate::models::EditKind) -> Style {
    use crate::models::EditKind;
    match kind {
        EditKind::Added => Style::default().fg(Color::Green),
        EditKind::Deleted => Style::default().fg(Color::Red),
        EditKind::Modified => Style::default().fg(Color::Yellow),
    }
}

// ---- pane styles ----

pub fn selection() -> Style {
    Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
}
pub fn active_pr() -> Style {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
}
pub fn title() -> Style {
    Style::default().fg(Color::Yellow)
}
pub fn border_focused() -> Style {
    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
}
/// Green + bold, used for focus accents (markers, labels).
pub fn focus() -> Style {
    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
}
pub fn border_dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}
pub fn status() -> Style {
    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
}
/// A hook run that failed: the border is the signal, since a run that passes
/// takes its window away with it.
pub fn hook_failed() -> Style {
    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
}

pub fn keys() -> Style {
    // Bottom action bar, lazygit-style: blue.
    Style::default().fg(Color::Blue)
}
/// Colored "pastille" bar marking the selected row in a list pane.
pub fn sel_marker() -> Style {
    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
}
pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}
/// List section header: bold, in a muted gray.
///
/// The gray comes from the color, not from `DIM`. A cell that carries both
/// `DIM` and `BOLD` renders differently depending on the order the backend
/// emits the two SGR codes, which changes with the redraw path.
pub fn section_header() -> Style {
    Style::default().fg(Color::Indexed(245)).add_modifier(Modifier::BOLD)
}

/// Style for a markdown line kind (PR summary pane).
pub fn kind_style(kind: Kind) -> Style {
    match kind {
        Kind::Title => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        Kind::Meta => Style::default().fg(Color::Cyan),
        Kind::Sep => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Kind::H1 => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        Kind::H2 => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        Kind::H3 => Style::default().add_modifier(Modifier::BOLD),
        Kind::Summary => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Kind::Quote | Kind::Rule | Kind::Dim => Style::default().add_modifier(Modifier::DIM),
        Kind::Code => Style::default().fg(Color::Cyan),
        Kind::Plain | Kind::Bullet => Style::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify() {
        assert_eq!(classify_diff_line("+added"), DiffKind::Add);
        assert_eq!(classify_diff_line("+++ b/f"), DiffKind::Meta);
        assert_eq!(classify_diff_line("-gone"), DiffKind::Del);
        assert_eq!(classify_diff_line("@@ -1 +1 @@"), DiffKind::Hunk);
        assert_eq!(classify_diff_line(" ctx"), DiffKind::Context);
    }

    #[test]
    fn the_env_var_wins_over_colorfgbg() {
        assert_eq!(detect_appearance(Some("light"), Some("0;15")), Appearance::Light);
        assert_eq!(detect_appearance(Some("Dark"), Some("0;15")), Appearance::Dark);
        assert_eq!(detect_appearance(Some("nonsense"), Some("15;0")), Appearance::Dark);
    }

    #[test]
    fn colorfgbg_reads_the_background_field() {
        assert_eq!(detect_appearance(None, Some("15;0")), Appearance::Dark);
        assert_eq!(detect_appearance(None, Some("15;8")), Appearance::Dark);
        assert_eq!(detect_appearance(None, Some("0;15")), Appearance::Light);
        assert_eq!(detect_appearance(None, Some("0;7")), Appearance::Light);
        // Some terminals write a third field between the two colors.
        assert_eq!(detect_appearance(None, Some("0;default;15")), Appearance::Light);
        assert_eq!(detect_appearance(None, Some("12;default;0")), Appearance::Dark);
        // Anything unreadable falls back to dark.
        assert_eq!(detect_appearance(None, Some("")), Appearance::Dark);
        assert_eq!(detect_appearance(None, Some("0;12")), Appearance::Dark);
        assert_eq!(detect_appearance(None, None), Appearance::Dark);
    }

    #[test]
    fn the_focused_block_keeps_its_own_tint() {
        for kind in [DiffKind::Add, DiffKind::Del] {
            let (plain, current) = (diff_row_style(kind, false), diff_row_style(kind, true));
            assert!(plain.bg.is_some() && current.bg.is_some());
            assert_ne!(plain.bg, current.bg);
        }
        assert!(diff_row_style(DiffKind::Context, true).bg.is_none());
    }

    #[test]
    fn current_hunk_bands_only_changed_lines() {
        // changed lines differ when current; context/header do not.
        assert_ne!(diff_line_style("+x", true), diff_line_style("+x", false));
        assert_eq!(diff_line_style(" ctx", true), diff_line_style(" ctx", false));
        assert_eq!(diff_line_style("@@ x @@", true), diff_line_style("@@ x @@", false));
    }
}
