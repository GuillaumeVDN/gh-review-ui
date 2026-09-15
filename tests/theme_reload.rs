//! The palette follows the active Omarchy theme, and a switch repaints.

use std::time::{Duration, Instant};

use ratatui::style::Color;

use ghreview::omarchy;
use ghreview::theme::{self, DiffKind, Token, Watch};

fn state_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ghreview-theme-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("omarchy/current/theme")).unwrap();
    dir
}

fn install(dir: &std::path::Path, theme: &str) {
    let src = format!("tests/data/{theme}.toml");
    std::fs::copy(src, dir.join("omarchy/current/theme/colors.toml")).unwrap();
    std::fs::write(dir.join("omarchy/current/theme.name"), theme).unwrap();
}

#[test]
fn a_theme_switch_repaints_without_a_restart() {
    let dir = state_dir("switch");
    install(&dir, "catppuccin-latte");
    std::env::set_var("XDG_STATE_HOME", &dir);
    std::env::remove_var("GH_REVIEW_UI_THEME");
    theme::reload();

    let latte = omarchy::load().unwrap();
    assert_eq!(theme::token_style(Token::Keyword).fg, Some(Color::Rgb(0xea, 0x76, 0xcb)));
    assert_eq!(theme::appearance(), theme::Appearance::Light);
    let light_add = theme::diff_row_style(DiffKind::Add, false).bg.unwrap();

    // The watch reports the switch, and the reload it runs changes the colors.
    let mut watch = Watch::default();
    install(&dir, "tokyo-night");
    let later = Instant::now() + Duration::from_secs(2);
    assert!(watch.changed(later, omarchy::changed_at));
    theme::reload();

    let tokyo = omarchy::load().unwrap();
    assert_ne!(tokyo.magenta, latte.magenta);
    assert_eq!(theme::token_style(Token::Keyword).fg, Some(Color::Rgb(0xad, 0x8e, 0xe6)));
    assert_eq!(theme::appearance(), theme::Appearance::Dark);
    assert_ne!(theme::diff_row_style(DiffKind::Add, false).bg.unwrap(), light_add);

    // Without a theme file the app keeps its own colors.
    std::fs::remove_file(dir.join("omarchy/current/theme/colors.toml")).unwrap();
    std::env::set_var("GH_REVIEW_UI_THEME", "dark");
    theme::reload();
    assert!(omarchy::load().is_none());
    assert_eq!(theme::token_style(Token::Keyword).fg, Some(Color::Blue));
    assert_eq!(theme::diff_row_style(DiffKind::Add, false).bg, Some(Color::Indexed(22)));
    std::fs::remove_dir_all(&dir).ok();
}
