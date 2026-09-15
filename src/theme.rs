//! Colors and diff/highlight styling (ratatui `Style`s).
//!
//! The active Omarchy theme drives every color here, so the diff reads like the
//! editor next to it. Without that theme the app keeps its own ANSI colors.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

use ratatui::style::{Color, Modifier, Style};

use crate::markdown::Kind;
use crate::omarchy::{self, Colors, Rgb};

/// Whether the terminal paints on a dark or a light ground.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Appearance {
    Dark,
    Light,
}

/// What a piece of code is, from its syntax scope.
///
/// The highlight cache holds these, not colors, so a theme switch repaints
/// without parsing anything again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Token {
    Plain,
    Comment,
    Str,
    StringEscape,
    Number,
    Boolean,
    Keyword,
    KeywordOperator,
    Import,
    FunctionDef,
    FunctionCall,
    FunctionBuiltin,
    Type,
    TypeBuiltin,
    Operator,
    Parameter,
    VariableBuiltin,
    Property,
    Tag,
    Punctuation,
    Invalid,
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// How far a diff tint carries its hue, and the contrast the code on it keeps.
const ADD_DEL_TINT: f32 = 0.18;
const CURRENT_TINT: f32 = 0.32;
const TINT_FLOOR: f32 = 5.5;
const CURRENT_FLOOR: f32 = 4.5;

fn channel(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(c: Rgb) -> f32 {
    0.2126 * channel(c.0) + 0.7152 * channel(c.1) + 0.0722 * channel(c.2)
}

/// WCAG contrast ratio, between 1 and 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

pub fn blend(base: Rgb, hue: Rgb, ratio: f32) -> Rgb {
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * ratio).round() as u8;
    (mix(base.0, hue.0), mix(base.1, hue.1), mix(base.2, hue.2))
}

/// Carry `hue` into `base` as far as `ratio`, and back off while `text` on the
/// result reads under `floor`. A theme of its own low contrast gets a fainter
/// tint rather than an unreadable line.
pub fn tint(base: Rgb, hue: Rgb, text: Rgb, ratio: f32, floor: f32) -> Rgb {
    let mut ratio = ratio;
    while ratio > 0.04 {
        let out = blend(base, hue, ratio);
        if contrast(text, out) >= floor {
            return out;
        }
        ratio -= 0.02;
    }
    blend(base, hue, 0.04)
}

/// The colors of the whole app, from the Omarchy theme when there is one.
#[derive(Clone, Debug)]
pub struct Palette {
    pub appearance: Appearance,
    theme: Option<Colors>,
    add_bg: Color,
    del_bg: Color,
    add_bg_current: Color,
    del_bg_current: Color,
}

impl Palette {
    /// The app's own colors, for a machine Omarchy does not theme.
    fn ansi(appearance: Appearance) -> Palette {
        let (add, del, add_cur, del_cur) = match appearance {
            Appearance::Dark => (22, 52, 28, 88),
            Appearance::Light => (194, 224, 151, 217),
        };
        Palette {
            appearance,
            theme: None,
            add_bg: Color::Indexed(add),
            del_bg: Color::Indexed(del),
            add_bg_current: Color::Indexed(add_cur),
            del_bg_current: Color::Indexed(del_cur),
        }
    }

    fn themed(c: Colors, forced: Option<Appearance>) -> Palette {
        let appearance = forced.unwrap_or(match c.light {
            true => Appearance::Light,
            false => Appearance::Dark,
        });
        let band = |hue: Rgb, ratio: f32, floor: f32| {
            rgb(tint(c.background, hue, c.foreground, ratio, floor))
        };
        Palette {
            appearance,
            add_bg: band(c.green, ADD_DEL_TINT, TINT_FLOOR),
            del_bg: band(c.red, ADD_DEL_TINT, TINT_FLOOR),
            add_bg_current: band(c.green, CURRENT_TINT, CURRENT_FLOOR),
            del_bg_current: band(c.red, CURRENT_TINT, CURRENT_FLOOR),
            theme: Some(c),
        }
    }

    /// One theme color, or the ANSI color that stands for it.
    fn color(&self, pick: fn(&Colors) -> Rgb, ansi: Color) -> Color {
        match &self.theme {
            Some(c) => rgb(pick(c)),
            None => ansi,
        }
    }

    fn fg(&self, pick: fn(&Colors) -> Rgb, ansi: Color) -> Style {
        Style::default().fg(self.color(pick, ansi))
    }

    /// The color of one piece of code.
    pub fn token(&self, token: Token) -> Style {
        match &self.theme {
            Some(c) => themed_token(c, token),
            None => ansi_token(token),
        }
    }

    /// The background a diff row sits on: a tint for a changed line, nothing
    /// for context and headers. The focused change block takes the deeper one.
    pub fn row(&self, kind: DiffKind, current: bool) -> Style {
        match (kind, current) {
            (DiffKind::Add, false) => Style::default().bg(self.add_bg),
            (DiffKind::Add, true) => Style::default().bg(self.add_bg_current),
            (DiffKind::Del, false) => Style::default().bg(self.del_bg),
            (DiffKind::Del, true) => Style::default().bg(self.del_bg_current),
            _ => Style::default(),
        }
    }

    /// The `+` / `-` of a changed line.
    pub fn marker(&self, kind: DiffKind, current: bool) -> Style {
        let fg = match kind {
            DiffKind::Add => self.color(|c| c.green, Color::Green),
            DiffKind::Del => self.color(|c| c.red, Color::Red),
            _ => return self.row(kind, current),
        };
        self.row(kind, current).fg(fg).add_modifier(Modifier::BOLD)
    }

    /// The line numbers of a diff row.
    pub fn number(&self, kind: DiffKind, current: bool) -> Style {
        match kind {
            DiffKind::Add => self.row(kind, current).fg(self.color(|c| c.green, Color::Green)),
            DiffKind::Del => self.row(kind, current).fg(self.color(|c| c.red, Color::Red)),
            _ => dim_of(self),
        }
    }

    /// The line the comment picker points at.
    pub fn selected(&self) -> Style {
        match &self.theme {
            Some(c) => Style::default().bg(rgb(c.selection)).fg(rgb(c.bright_foreground)),
            None => Style::default().add_modifier(Modifier::REVERSED),
        }
    }
}

fn themed_token(c: &Colors, token: Token) -> Style {
    let plain = |x: Rgb| Style::default().fg(rgb(x));
    let bold = |x: Rgb| plain(x).add_modifier(Modifier::BOLD);
    let italic = |x: Rgb| plain(x).add_modifier(Modifier::ITALIC);
    match token {
        Token::Comment => italic(c.dark_foreground),
        Token::Str => plain(c.green),
        Token::StringEscape => bold(c.orange),
        Token::Number => plain(c.orange),
        Token::Boolean => bold(c.orange),
        Token::Keyword => bold(c.magenta),
        Token::KeywordOperator => bold(c.cyan),
        Token::Import => bold(c.cyan),
        Token::FunctionDef => bold(c.blue),
        Token::FunctionCall => plain(c.blue),
        Token::FunctionBuiltin => bold(c.cyan),
        Token::Type => bold(c.yellow),
        Token::TypeBuiltin => italic(c.yellow),
        Token::Operator => plain(c.cyan),
        Token::Parameter => italic(c.foreground),
        Token::VariableBuiltin => italic(c.red),
        Token::Property => plain(c.blue),
        Token::Tag => plain(c.blue),
        Token::Invalid => bold(c.red),
        Token::Punctuation | Token::Plain => plain(c.foreground),
    }
}

/// Green and red stay out of the ANSI set: there they mean added and deleted.
fn ansi_token(token: Token) -> Style {
    let plain = |c: Color| Style::default().fg(c);
    match token {
        Token::Comment => plain(Color::DarkGray),
        Token::Str | Token::StringEscape => plain(Color::Yellow),
        Token::Number | Token::Boolean => plain(Color::Magenta),
        Token::Keyword | Token::KeywordOperator | Token::Import => {
            plain(Color::Blue).add_modifier(Modifier::BOLD)
        }
        Token::FunctionDef | Token::FunctionCall | Token::FunctionBuiltin => plain(Color::Cyan),
        Token::Type | Token::TypeBuiltin => plain(Color::LightCyan),
        _ => Style::default(),
    }
}

/// `GH_REVIEW_UI_THEME`, when it names an appearance.
pub fn forced_appearance() -> Option<Appearance> {
    match std::env::var("GH_REVIEW_UI_THEME").ok()?.trim().to_ascii_lowercase().as_str() {
        "light" => Some(Appearance::Light),
        "dark" => Some(Appearance::Dark),
        _ => None,
    }
}

/// The appearance in an OSC 11 reply: `\x1b]11;rgb:rrrr/gggg/bbbb` closed by
/// `\x1b\\` or BEL. Components carry one to four hex digits.
pub fn parse_osc11(reply: &str) -> Option<Appearance> {
    let body = reply.split("]11;").nth(1)?;
    let body = body.split(['\x07', '\x1b']).next()?;
    let spec = body.trim().strip_prefix("rgb:")?;
    let mut parts = spec.split('/').map(hex_channel);
    let (r, g, b) = (parts.next()??, parts.next()??, parts.next()??);
    let light = 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5;
    Some(if light { Appearance::Light } else { Appearance::Dark })
}

/// One hex component of an X11 color, as a fraction of its own full scale.
fn hex_channel(part: &str) -> Option<f32> {
    let digits = part.len();
    if digits == 0 || digits > 4 || !part.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full = (1u32 << (4 * digits)) - 1;
    Some(u32::from_str_radix(part, 16).ok()? as f32 / full as f32)
}

/// The terminal appearance, for a machine with no Omarchy theme:
/// `GH_REVIEW_UI_THEME`, then what the terminal answered about its background,
/// then `COLORFGBG`, then dark.
///
/// `COLORFGBG` holds `fg;bg`, and some terminals put a third field between the
/// two. The background index is the last field.
pub fn detect_appearance(
    forced: Option<Appearance>,
    queried: Option<Appearance>,
    colorfgbg: Option<&str>,
) -> Appearance {
    if let Some(a) = forced.or(queried) {
        return a;
    }
    let bg = colorfgbg
        .and_then(|v| v.rsplit(';').next())
        .and_then(|v| v.trim().parse::<u8>().ok());
    match bg {
        Some(7) | Some(15) => Appearance::Light,
        _ => Appearance::Dark,
    }
}

static QUERIED: OnceLock<Option<Appearance>> = OnceLock::new();
static ACTIVE: RwLock<Option<Arc<Palette>>> = RwLock::new(None);

/// Settle the colors, with what the terminal answered about its background.
pub fn init_appearance(queried: Option<Appearance>) {
    let _ = QUERIED.set(queried);
    reload();
}

/// Read the theme again, after a theme switch.
pub fn reload() {
    let built = Arc::new(build());
    if let Ok(mut active) = ACTIVE.write() {
        *active = Some(built);
    }
}

fn build() -> Palette {
    let forced = forced_appearance();
    match omarchy::load() {
        Some(colors) => Palette::themed(colors, forced),
        None => Palette::ansi(detect_appearance(
            forced,
            *QUERIED.get().unwrap_or(&None),
            std::env::var("COLORFGBG").ok().as_deref(),
        )),
    }
}

pub fn palette() -> Arc<Palette> {
    if let Some(found) = ACTIVE.read().ok().and_then(|active| active.clone()) {
        return found;
    }
    let built = Arc::new(build());
    if let Ok(mut active) = ACTIVE.write() {
        *active = Some(built.clone());
    }
    built
}

pub fn appearance() -> Appearance {
    palette().appearance
}

/// How often the event loop looks at the theme file.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// Watches the active theme, so switching it repaints the app.
pub struct Watch {
    checked: Instant,
    stamp: Option<SystemTime>,
}

impl Default for Watch {
    fn default() -> Watch {
        Watch { checked: Instant::now(), stamp: omarchy::changed_at() }
    }
}

impl Watch {
    /// Whether the theme changed since the last look. It looks at the file
    /// once a second at most, and the first answer after a switch is the only
    /// one that says yes.
    pub fn changed(&mut self, now: Instant, stamp: impl FnOnce() -> Option<SystemTime>) -> bool {
        if now.duration_since(self.checked) < WATCH_EVERY {
            return false;
        }
        self.checked = now;
        let found = stamp();
        if found == self.stamp {
            return false;
        }
        self.stamp = found;
        true
    }

    /// Read the theme again when it changed, and say whether it did.
    pub fn poll(&mut self) -> bool {
        if !self.changed(Instant::now(), omarchy::changed_at) {
            return false;
        }
        reload();
        true
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

pub fn diff_row_style(kind: DiffKind, current: bool) -> Style {
    palette().row(kind, current)
}

pub fn diff_marker_style(kind: DiffKind, current: bool) -> Style {
    palette().marker(kind, current)
}

pub fn diff_number_style(kind: DiffKind, current: bool) -> Style {
    palette().number(kind, current)
}

pub fn token_style(token: Token) -> Style {
    palette().token(token)
}

/// The line the comment picker points at.
pub fn picked() -> Style {
    palette().selected()
}

/// Style for a whole diff line, code included. The panes that color the code
/// itself use [`diff_row_style`] and the token colors instead.
pub fn diff_line_style(line: &str, current: bool) -> Style {
    let p = palette();
    let k = classify_diff_line(line);
    match k {
        DiffKind::Add | DiffKind::Del => p.marker(k, current),
        DiffKind::Hunk => p.fg(|c| c.cyan, Color::Cyan).add_modifier(Modifier::BOLD),
        DiffKind::Meta => match p.theme {
            Some(_) => p.fg(|c| c.dark_foreground, Color::Reset),
            None => Style::default().add_modifier(Modifier::BOLD),
        },
        DiffKind::Context => Style::default(),
    }
}

/// Green + bold, or the theme's accent: focus accents (markers, labels).
pub fn focus() -> Style {
    palette().fg(|c| c.accent, Color::Green).add_modifier(Modifier::BOLD)
}

pub fn hunk_marker() -> Style {
    focus()
}

pub fn comment_marker() -> Style {
    focus()
}

/// Inline pending-comment body shown under a diff line.
pub fn comment_inline() -> Style {
    palette().fg(|c| c.cyan, Color::Cyan)
}

/// Local (uncommitted worktree) edits overlaid on the PR diff, in orange.
fn local() -> Style {
    palette().fg(|c| c.orange, Color::Indexed(208))
}

pub fn local_add() -> Style {
    local()
}
pub fn local_del() -> Style {
    local().add_modifier(Modifier::CROSSED_OUT)
}
pub fn local_marker() -> Style {
    local().add_modifier(Modifier::BOLD)
}

/// A commit that is on no remote, in the same orange as the uncommitted edits:
/// both are local work, still yours to rewrite, and neither is on the PR.
pub fn unpushed_commit() -> Style {
    local()
}

/// Row style for a pending-edit entry, by change kind.
pub fn edit_kind_style(kind: crate::models::EditKind) -> Style {
    use crate::models::EditKind;
    let p = palette();
    match kind {
        EditKind::Added => p.fg(|c| c.green, Color::Green),
        EditKind::Deleted => p.fg(|c| c.red, Color::Red),
        EditKind::Modified => p.fg(|c| c.yellow, Color::Yellow),
    }
}

// ---- pane styles ----

pub fn selection() -> Style {
    let p = palette();
    match &p.theme {
        Some(c) => Style::default().fg(rgb(c.background)).bg(rgb(c.accent)),
        None => Style::default().fg(Color::Black).bg(Color::Green),
    }
    .add_modifier(Modifier::BOLD)
}
pub fn active_pr() -> Style {
    palette().fg(|c| c.cyan, Color::Cyan).add_modifier(Modifier::BOLD)
}
pub fn title() -> Style {
    palette().fg(|c| c.yellow, Color::Yellow)
}
pub fn border_focused() -> Style {
    focus()
}
pub fn border_dim() -> Style {
    let p = palette();
    match &p.theme {
        Some(c) => Style::default().fg(rgb(c.muted)),
        None => Style::default().add_modifier(Modifier::DIM),
    }
}
pub fn status() -> Style {
    palette().fg(|c| c.yellow, Color::Yellow).add_modifier(Modifier::BOLD)
}
/// A hook run that failed: the border is the signal, since a run that passes
/// takes its window away with it.
pub fn hook_failed() -> Style {
    palette().fg(|c| c.red, Color::Red).add_modifier(Modifier::BOLD)
}

pub fn keys() -> Style {
    // Bottom action bar, lazygit-style: blue.
    palette().fg(|c| c.blue, Color::Blue)
}
/// Colored "pastille" bar marking the selected row in a list pane.
pub fn sel_marker() -> Style {
    focus()
}

fn dim_of(p: &Palette) -> Style {
    match &p.theme {
        Some(c) => Style::default().fg(rgb(c.dark_foreground)),
        None => Style::default().add_modifier(Modifier::DIM),
    }
}

pub fn dim() -> Style {
    dim_of(&palette())
}

/// List section header: bold, in a muted gray.
///
/// The gray comes from the color, not from `DIM`. A cell that carries both
/// `DIM` and `BOLD` renders differently depending on the order the backend
/// emits the two SGR codes, which changes with the redraw path.
pub fn section_header() -> Style {
    palette().fg(|c| c.dark_foreground, Color::Indexed(245)).add_modifier(Modifier::BOLD)
}

/// Style for a markdown line kind (PR summary pane).
pub fn kind_style(kind: Kind) -> Style {
    let p = palette();
    let cyan = || p.fg(|c| c.cyan, Color::Cyan);
    let yellow = || p.fg(|c| c.yellow, Color::Yellow);
    match kind {
        Kind::Title => focus(),
        Kind::Meta => cyan(),
        Kind::Sep => cyan().add_modifier(Modifier::BOLD),
        Kind::H1 => yellow().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        Kind::H2 => yellow().add_modifier(Modifier::BOLD),
        Kind::H3 => Style::default().add_modifier(Modifier::BOLD),
        Kind::Summary => cyan().add_modifier(Modifier::BOLD),
        Kind::Quote | Kind::Rule | Kind::Dim => dim_of(&p),
        Kind::Code => p.fg(|c| c.green, Color::Cyan),
        Kind::Plain | Kind::Bullet => Style::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATTE: &str = include_str!("../tests/data/catppuccin-latte.toml");
    const TOKYO: &str = include_str!("../tests/data/tokyo-night.toml");

    fn latte() -> Palette {
        Palette::themed(Colors::from_toml(LATTE).unwrap(), None)
    }

    #[test]
    fn classify() {
        assert_eq!(classify_diff_line("+added"), DiffKind::Add);
        assert_eq!(classify_diff_line("+++ b/f"), DiffKind::Meta);
        assert_eq!(classify_diff_line("-gone"), DiffKind::Del);
        assert_eq!(classify_diff_line("@@ -1 +1 @@"), DiffKind::Hunk);
        assert_eq!(classify_diff_line(" ctx"), DiffKind::Context);
    }

    #[test]
    fn the_answers_come_in_order() {
        let (dark, light) = (Some(Appearance::Dark), Some(Appearance::Light));
        // The env var wins over everything.
        assert_eq!(detect_appearance(light, dark, Some("15;0")), Appearance::Light);
        assert_eq!(detect_appearance(dark, light, Some("0;15")), Appearance::Dark);
        // Then what the terminal answered.
        assert_eq!(detect_appearance(None, light, Some("15;0")), Appearance::Light);
        assert_eq!(detect_appearance(None, dark, Some("0;15")), Appearance::Dark);
        // Then COLORFGBG, then dark.
        assert_eq!(detect_appearance(None, None, Some("0;15")), Appearance::Light);
        assert_eq!(detect_appearance(None, None, None), Appearance::Dark);
    }

    #[test]
    fn colorfgbg_reads_the_background_field() {
        assert_eq!(detect_appearance(None, None, Some("15;0")), Appearance::Dark);
        assert_eq!(detect_appearance(None, None, Some("15;8")), Appearance::Dark);
        assert_eq!(detect_appearance(None, None, Some("0;15")), Appearance::Light);
        assert_eq!(detect_appearance(None, None, Some("0;7")), Appearance::Light);
        // Some terminals write a third field between the two colors.
        assert_eq!(detect_appearance(None, None, Some("0;default;15")), Appearance::Light);
        assert_eq!(detect_appearance(None, None, Some("12;default;0")), Appearance::Dark);
        // Anything unreadable falls back to dark.
        assert_eq!(detect_appearance(None, None, Some("")), Appearance::Dark);
        assert_eq!(detect_appearance(None, None, Some("0;12")), Appearance::Dark);
    }

    #[test]
    fn an_osc11_reply_gives_the_appearance() {
        // Both terminators, and components of any width.
        assert_eq!(parse_osc11("\x1b]11;rgb:ffff/ffff/ffff\x1b\\"), Some(Appearance::Light));
        assert_eq!(parse_osc11("\x1b]11;rgb:0000/0000/0000\x07"), Some(Appearance::Dark));
        assert_eq!(parse_osc11("\x1b]11;rgb:fa/f8/ef\x07"), Some(Appearance::Light));
        assert_eq!(parse_osc11("\x1b]11;rgb:1d/1f/21\x1b\\"), Some(Appearance::Dark));
        assert_eq!(parse_osc11("\x1b]11;rgb:f/f/f\x07"), Some(Appearance::Light));
        // Green carries most of the luminance.
        assert_eq!(parse_osc11("\x1b]11;rgb:0000/ffff/0000\x07"), Some(Appearance::Light));
        assert_eq!(parse_osc11("\x1b]11;rgb:ffff/0000/0000\x07"), Some(Appearance::Dark));
    }

    #[test]
    fn a_reply_that_says_nothing_is_no_answer() {
        assert_eq!(parse_osc11(""), None);
        assert_eq!(parse_osc11("\x1b]11;"), None);
        assert_eq!(parse_osc11("\x1b]11;rgb:1d/1f\x07"), None);
        assert_eq!(parse_osc11("\x1b]11;rgb:zz/1f/21\x07"), None);
        assert_eq!(parse_osc11("\x1b]11;rgb:11111/1f/21\x07"), None);
        assert_eq!(parse_osc11("\x1b]11;#1d1f21\x07"), None);
        assert_eq!(parse_osc11("\x1b]10;rgb:0000/0000/0000\x07"), None);
        assert_eq!(parse_osc11("some key presses"), None);
    }

    #[test]
    fn blending_walks_from_one_color_to_the_other() {
        let (black, white) = ((0, 0, 0), (255, 255, 255));
        assert_eq!(blend(black, white, 0.0), black);
        assert_eq!(blend(black, white, 1.0), white);
        assert_eq!(blend(black, white, 0.5), (128, 128, 128));
        assert!((contrast(black, white) - 21.0).abs() < 0.01);
        assert!((contrast(white, white) - 1.0).abs() < 0.01);
    }

    #[test]
    fn a_tint_backs_off_until_the_code_reads() {
        let (bg, fg, hue) = ((0xef, 0xf1, 0xf5), (0x4c, 0x4f, 0x69), (0xd2, 0x0f, 0x39));
        let deep = tint(bg, hue, fg, 1.0, 5.5);
        assert!(contrast(fg, deep) >= 5.5, "{:?}", contrast(fg, deep));
        // A floor nothing can meet still leaves a visible tint.
        let faint = tint(bg, hue, fg, 0.5, 21.0);
        assert_eq!(faint, blend(bg, hue, 0.04));
    }

    #[test]
    fn every_omarchy_theme_keeps_its_diff_readable() {
        for text in [LATTE, TOKYO] {
            let c = Colors::from_toml(text).unwrap();
            let p = Palette::themed(c.clone(), None);
            for band in [p.add_bg, p.del_bg] {
                let Color::Rgb(r, g, b) = band else { panic!("a themed band is rgb") };
                assert!(contrast(c.foreground, (r, g, b)) >= TINT_FLOOR);
            }
            for band in [p.add_bg_current, p.del_bg_current] {
                let Color::Rgb(r, g, b) = band else { panic!("a themed band is rgb") };
                assert!(contrast(c.foreground, (r, g, b)) >= CURRENT_FLOOR);
            }
            assert_ne!(p.add_bg, p.add_bg_current);
            assert_ne!(p.del_bg, p.del_bg_current);
        }
    }

    #[test]
    fn the_theme_paints_the_tokens_like_the_editor() {
        let p = latte();
        let c = Colors::from_toml(LATTE).unwrap();
        assert_eq!(p.token(Token::Str).fg, Some(rgb(c.green)));
        assert_eq!(p.token(Token::Keyword).fg, Some(rgb(c.magenta)));
        assert!(p.token(Token::Keyword).add_modifier.contains(Modifier::BOLD));
        assert_eq!(p.token(Token::Comment).fg, Some(rgb(c.dark_foreground)));
        assert!(p.token(Token::Comment).add_modifier.contains(Modifier::ITALIC));
        assert_eq!(p.token(Token::FunctionDef).fg, Some(rgb(c.blue)));
        assert_eq!(p.token(Token::Type).fg, Some(rgb(c.yellow)));
        assert_eq!(p.token(Token::VariableBuiltin).fg, Some(rgb(c.red)));
        assert_eq!(p.token(Token::Number).fg, Some(rgb(c.orange)));
        assert_eq!(p.token(Token::Import).fg, Some(rgb(c.cyan)));
        assert_eq!(p.token(Token::Plain).fg, Some(rgb(c.foreground)));
    }

    #[test]
    fn the_ansi_palette_keeps_green_and_red_for_the_diff() {
        let p = Palette::ansi(Appearance::Dark);
        for token in [
            Token::Comment,
            Token::Str,
            Token::Number,
            Token::Keyword,
            Token::FunctionDef,
            Token::Type,
            Token::Plain,
        ] {
            assert!(!matches!(p.token(token).fg, Some(Color::Green) | Some(Color::Red)));
        }
        assert_eq!(p.token(Token::Str).fg, Some(Color::Yellow));
        assert!(p.selected().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn the_picker_sits_on_the_theme_selection() {
        let p = latte();
        let c = Colors::from_toml(LATTE).unwrap();
        assert_eq!(p.selected().bg, Some(rgb(c.selection)));
        assert!(!p.selected().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn the_env_var_still_names_the_mode() {
        let dark = Palette::themed(Colors::from_toml(LATTE).unwrap(), Some(Appearance::Dark));
        assert_eq!(dark.appearance, Appearance::Dark);
        assert_eq!(latte().appearance, Appearance::Light);
    }

    #[test]
    fn the_focused_block_keeps_its_own_tint() {
        for p in [latte(), Palette::ansi(Appearance::Dark)] {
            for kind in [DiffKind::Add, DiffKind::Del] {
                let (plain, current) = (p.row(kind, false), p.row(kind, true));
                assert!(plain.bg.is_some() && current.bg.is_some());
                assert_ne!(plain.bg, current.bg);
            }
            assert!(p.row(DiffKind::Context, true).bg.is_none());
        }
    }

    #[test]
    fn the_watch_looks_once_a_second() {
        let start = Instant::now();
        let mut watch = Watch { checked: start, stamp: Some(SystemTime::UNIX_EPOCH) };
        let later = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);

        // Too soon to look, whatever the file says.
        assert!(!watch.changed(start, || Some(later(9))));
        // A second later the new stamp answers once.
        let now = start + WATCH_EVERY;
        assert!(watch.changed(now, || Some(later(9))));
        let now = now + WATCH_EVERY;
        assert!(!watch.changed(now, || Some(later(9))));
        // A file that goes missing is a change too.
        let now = now + WATCH_EVERY;
        assert!(watch.changed(now, || None));
        let now = now + WATCH_EVERY;
        assert!(!watch.changed(now, || None));
    }

    #[test]
    fn current_hunk_bands_only_changed_lines() {
        // changed lines differ when current; context/header do not.
        assert_ne!(diff_line_style("+x", true), diff_line_style("+x", false));
        assert_eq!(diff_line_style(" ctx", true), diff_line_style(" ctx", false));
        assert_eq!(diff_line_style("@@ x @@", true), diff_line_style("@@ x @@", false));
    }
}
