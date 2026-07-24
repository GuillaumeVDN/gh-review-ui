//! Colors and diff/highlight styling (ratatui `Style`s).

use ratatui::style::{Color, Modifier, Style};

use crate::markdown::Kind;

// Highlighted-hunk band: dark fixed fg on a light cyan bg so +/- text stays
// readable regardless of how the terminal theme remaps the base green/red.
const HL_BG: Color = Color::Indexed(152); // light cyan
const HL_ADD: Color = Color::Indexed(22); // dark green
const HL_DEL: Color = Color::Indexed(88); // dark red

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

/// Style for a diff line. In the focused hunk only actual changed (+/-) lines
/// get the background band; context/header keep normal styling.
pub fn diff_line_style(line: &str, current: bool) -> Style {
    let k = classify_diff_line(line);
    if current && matches!(k, DiffKind::Add | DiffKind::Del) {
        return match k {
            DiffKind::Add => Style::default().fg(HL_ADD).bg(HL_BG),
            _ => Style::default().fg(HL_DEL).bg(HL_BG),
        };
    }
    match k {
        DiffKind::Add => Style::default().fg(Color::Green),
        DiffKind::Del => Style::default().fg(Color::Red),
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
pub fn keys() -> Style {
    Style::default().fg(Color::White).add_modifier(Modifier::DIM)
}
pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}
pub fn section_header() -> Style {
    Style::default().fg(Color::White).add_modifier(Modifier::DIM | Modifier::BOLD)
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
    fn current_hunk_bands_only_changed_lines() {
        // changed lines differ when current; context/header do not.
        assert_ne!(diff_line_style("+x", true), diff_line_style("+x", false));
        assert_eq!(diff_line_style(" ctx", true), diff_line_style(" ctx", false));
        assert_eq!(diff_line_style("@@ x @@", true), diff_line_style("@@ x @@", false));
    }
}
