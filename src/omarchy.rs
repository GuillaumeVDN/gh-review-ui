//! The active Omarchy theme, read from the state directory it publishes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// An `#rrggbb` color of a theme file.
pub type Rgb = (u8, u8, u8);

/// The palette Omarchy writes for every app it themes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Colors {
    pub light: bool,
    pub accent: Rgb,
    pub selection: Rgb,
    pub muted: Rgb,
    pub background: Rgb,
    pub foreground: Rgb,
    pub dark_foreground: Rgb,
    pub bright_foreground: Rgb,
    pub red: Rgb,
    pub yellow: Rgb,
    pub orange: Rgb,
    pub green: Rgb,
    pub cyan: Rgb,
    pub blue: Rgb,
    pub magenta: Rgb,
}

fn state_dir() -> PathBuf {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"),
    }
}

fn current_dir() -> PathBuf {
    state_dir().join("omarchy/current")
}

pub fn colors_path() -> PathBuf {
    current_dir().join("theme/colors.toml")
}

/// What the theme was last changed at, for the reload check. The name file is
/// what Omarchy rewrites on a switch; the palette answers when it is absent.
pub fn changed_at() -> Option<SystemTime> {
    let name = current_dir().join("theme.name");
    let file = if name.exists() { name } else { colors_path() };
    std::fs::metadata(file).ok()?.modified().ok()
}

/// Read `key = "value"` lines. The theme files hold nothing else, so a line
/// that is not one is skipped.
pub fn parse_toml(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
            continue;
        };
        let key = key.trim();
        if !key.is_empty() {
            out.insert(key.to_string(), value.to_string());
        }
    }
    out
}

pub fn parse_hex(value: &str) -> Option<Rgb> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

impl Colors {
    /// Build a palette from the file's keys. A missing or malformed color
    /// answers `None`, and the app keeps its own colors.
    pub fn from_toml(text: &str) -> Option<Colors> {
        let map = parse_toml(text);
        let color = |key: &str| map.get(key).and_then(|v| parse_hex(v));
        Some(Colors {
            light: map.get("mode").map(String::as_str) == Some("light"),
            accent: color("accent")?,
            selection: color("selection")?,
            muted: color("muted")?,
            background: color("background")?,
            foreground: color("foreground")?,
            dark_foreground: color("dark_foreground")?,
            bright_foreground: color("bright_foreground")?,
            red: color("red")?,
            yellow: color("yellow")?,
            orange: color("orange")?,
            green: color("green")?,
            cyan: color("cyan")?,
            blue: color("blue")?,
            magenta: color("magenta")?,
        })
    }
}

pub fn load() -> Option<Colors> {
    Colors::from_toml(&std::fs::read_to_string(colors_path()).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATTE: &str = include_str!("../tests/data/catppuccin-latte.toml");

    #[test]
    fn a_theme_file_gives_a_palette() {
        let c = Colors::from_toml(LATTE).unwrap();
        assert!(c.light);
        assert_eq!(c.background, (0xef, 0xf1, 0xf5));
        assert_eq!(c.green, (0x40, 0xa0, 0x2b));
        assert_eq!(c.accent, (0x1e, 0x66, 0xf5));
    }

    #[test]
    fn a_missing_color_gives_no_palette() {
        let text = LATTE.replace("green = \"#40a02b\"\n", "");
        assert!(Colors::from_toml(&text).is_none());
    }

    #[test]
    fn garbage_gives_no_palette() {
        assert!(Colors::from_toml("").is_none());
        assert!(Colors::from_toml("not a theme at all").is_none());
        assert!(Colors::from_toml(&LATTE.replace("#40a02b", "green")).is_none());
        assert!(Colors::from_toml(&LATTE.replace("#40a02b", "#40a02")).is_none());
    }

    #[test]
    fn only_quoted_pairs_are_read() {
        let map = parse_toml("# a comment\n[section]\nmode = \"dark\"\nbare = value\nempty\n a = \"b\" \n");
        assert_eq!(map.get("mode").map(String::as_str), Some("dark"));
        assert_eq!(map.get("a").map(String::as_str), Some("b"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn hex_colors_only() {
        assert_eq!(parse_hex("#0a1b2c"), Some((0x0a, 0x1b, 0x2c)));
        assert_eq!(parse_hex("#FFFFFF"), Some((255, 255, 255)));
        assert_eq!(parse_hex("0a1b2c"), None);
        assert_eq!(parse_hex("#0a1b2"), None);
        assert_eq!(parse_hex("#0a1b2cd"), None);
        assert_eq!(parse_hex("#zzzzzz"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn the_light_flag_needs_the_word() {
        let dark = LATTE.replace("mode = \"light\"", "mode = \"dark\"");
        assert!(!Colors::from_toml(&dark).unwrap().light);
        let none = LATTE.replace("mode = \"light\"", "");
        assert!(!Colors::from_toml(&none).unwrap().light);
    }
}
