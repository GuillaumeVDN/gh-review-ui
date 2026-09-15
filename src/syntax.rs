//! Syntax highlighting of diff code, mapped onto the terminal palette.
//!
//! A hunk is a fragment of a file, and its two sides are two different texts.
//! Each file is parsed twice: an "old" stream of the context and deleted lines,
//! a "new" stream of the context and added lines. Both restart at every `@@`
//! header, because the lines between two hunks are missing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::theme::{classify_diff_line, DiffKind};

/// One diff line's code, split into syntax-colored pieces. The `+`/`-` marker
/// is not part of it: the gutter and the background carry the diff semantics.
/// An empty list means "no colors", and the caller draws the line plain.
pub type StyledLine = Vec<(Style, String)>;

/// A diff longer than this keeps its plain colors. A parse costs about a tenth
/// of a second per thousand lines, and a file that big is skimmed, not read.
const MAX_LINES: usize = 4_000;

/// Files kept in the cache before it is dropped and rebuilt on demand.
const MAX_CACHED: usize = 64;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

struct Rule {
    scope: Scope,
    style: Style,
}

/// Scope prefixes, most specific first, and the color each one gets.
///
/// Green and red are left out on purpose: they mean added and deleted here.
fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let scope = |name: &str| Scope::new(name).expect("static scope name");
        let plain = |c: Color| Style::default().fg(c);
        let bold = |c: Color| Style::default().fg(c).add_modifier(Modifier::BOLD);
        vec![
            Rule { scope: scope("comment"), style: plain(Color::DarkGray) },
            Rule { scope: scope("string"), style: plain(Color::Yellow) },
            Rule { scope: scope("constant"), style: plain(Color::Magenta) },
            Rule { scope: scope("keyword"), style: bold(Color::Blue) },
            Rule { scope: scope("storage"), style: bold(Color::Blue) },
            Rule { scope: scope("entity.name.function"), style: plain(Color::Cyan) },
            Rule { scope: scope("support.function"), style: plain(Color::Cyan) },
            Rule { scope: scope("entity.name"), style: plain(Color::LightCyan) },
            Rule { scope: scope("support.type"), style: plain(Color::LightCyan) },
            Rule { scope: scope("support.class"), style: plain(Color::LightCyan) },
        ]
    })
}

/// The color of a token, from the innermost scope that a rule matches.
pub fn scope_style(stack: &ScopeStack) -> Style {
    for scope in stack.scopes.iter().rev() {
        if let Some(rule) = rules().iter().find(|r| r.scope.is_prefix_of(*scope)) {
            return rule.style;
        }
    }
    Style::default()
}

/// The code of a diff row, tabs expanded, or `None` for a header row.
fn content_of(line: &str) -> Option<String> {
    if line.starts_with('\\') {
        return None; // "\ No newline at end of file"
    }
    match classify_diff_line(line) {
        DiffKind::Add | DiffKind::Del => Some(line[1..].replace('\t', "    ")),
        DiffKind::Context => Some(line.strip_prefix(' ').unwrap_or(line).replace('\t', "    ")),
        DiffKind::Hunk | DiffKind::Meta => None,
    }
}

fn syntax_for<'a>(set: &'a SyntaxSet, path: &str, first: Option<&str>) -> Option<&'a SyntaxReference> {
    let file = std::path::Path::new(path);
    let ext = file.extension().and_then(|e| e.to_str());
    let name = file.file_name().and_then(|e| e.to_str());
    ext.and_then(|e| set.find_syntax_by_extension(e))
        .or_else(|| name.and_then(|n| set.find_syntax_by_extension(n)))
        .or_else(|| first.and_then(|l| set.find_syntax_by_first_line(l)))
}

/// One side of a file, parsed line by line.
struct Stream {
    parse: ParseState,
    stack: ScopeStack,
}

impl Stream {
    fn new(syntax: &SyntaxReference) -> Self {
        Stream { parse: ParseState::new(syntax), stack: ScopeStack::new() }
    }

    fn feed(&mut self, text: &str) -> StyledLine {
        // The default syntaxes match on a trailing newline.
        let line = format!("{text}\n");
        let Ok(ops) = self.parse.parse_line(&line, syntaxes()) else {
            return StyledLine::new();
        };
        let end = text.len();
        let mut out = StyledLine::new();
        let mut last = 0usize;
        for (at, op) in ops {
            let at = at.min(end);
            if at > last {
                push(&mut out, scope_style(&self.stack), &line[last..at]);
                last = at;
            }
            if self.stack.apply(&op).is_err() {
                return StyledLine::new();
            }
        }
        if last < end {
            push(&mut out, scope_style(&self.stack), &line[last..end]);
        }
        out
    }
}

fn push(out: &mut StyledLine, style: Style, text: &str) {
    match out.last_mut() {
        Some((s, t)) if *s == style => t.push_str(text),
        _ => out.push((style, text.to_string())),
    }
}

/// Color every code row of one file's diff, indexed like `lines`.
pub fn highlight_diff(path: &str, lines: &[String]) -> Vec<StyledLine> {
    if lines.len() > MAX_LINES {
        return Vec::new();
    }
    let set = syntaxes();
    let first = lines.iter().find_map(|l| content_of(l));
    let Some(syntax) = syntax_for(set, path, first.as_deref()) else {
        return Vec::new();
    };
    let mut out = vec![StyledLine::new(); lines.len()];
    let mut old = Stream::new(syntax);
    let mut new = Stream::new(syntax);
    for (i, line) in lines.iter().enumerate() {
        let kind = classify_diff_line(line);
        if kind == DiffKind::Hunk {
            old = Stream::new(syntax);
            new = Stream::new(syntax);
            continue;
        }
        let Some(text) = content_of(line) else { continue };
        out[i] = match kind {
            DiffKind::Add => new.feed(&text),
            DiffKind::Del => old.feed(&text),
            _ => {
                old.feed(&text);
                new.feed(&text)
            }
        };
    }
    out
}

fn fingerprint(lines: &[String]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lines.len().hash(&mut h);
    for l in lines {
        l.hash(&mut h);
    }
    h.finish()
}

type Cache = HashMap<(String, u64), Rc<Vec<StyledLine>>>;

/// Per-file highlight cache, so a redraw never parses anything.
///
/// The key holds the diff text's fingerprint, so a reloaded file is parsed
/// again and the two columns of a split diff each keep their own colors.
#[derive(Default)]
pub struct Highlighter {
    cache: RefCell<Cache>,
}

impl Highlighter {
    pub fn file(&self, path: &str, lines: &[String]) -> Rc<Vec<StyledLine>> {
        let key = (path.to_string(), fingerprint(lines));
        if let Some(hit) = self.cache.borrow().get(&key) {
            return hit.clone();
        }
        let built = Rc::new(highlight_diff(path, lines));
        let mut cache = self.cache.borrow_mut();
        if cache.len() >= MAX_CACHED {
            cache.clear();
        }
        cache.insert(key, built.clone());
        built
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(names: &[&str]) -> ScopeStack {
        let mut s = ScopeStack::new();
        for n in names {
            s.scopes.push(Scope::new(n).unwrap());
        }
        s
    }

    #[test]
    fn scopes_map_to_terminal_colors() {
        assert_eq!(scope_style(&stack(&["source.rs", "comment.line.rust"])).fg, Some(Color::DarkGray));
        assert_eq!(scope_style(&stack(&["source.rs", "string.quoted.double"])).fg, Some(Color::Yellow));
        assert_eq!(scope_style(&stack(&["source.rs", "constant.numeric"])).fg, Some(Color::Magenta));
        assert_eq!(scope_style(&stack(&["source.rs", "keyword.control"])).fg, Some(Color::Blue));
        assert_eq!(scope_style(&stack(&["source.rs", "storage.type"])).fg, Some(Color::Blue));
        assert_eq!(scope_style(&stack(&["source.rs", "entity.name.function"])).fg, Some(Color::Cyan));
        assert_eq!(scope_style(&stack(&["source.rs", "entity.name.class"])).fg, Some(Color::LightCyan));
        assert_eq!(scope_style(&stack(&["source.rs", "variable.parameter"])).fg, None);
        assert_eq!(scope_style(&stack(&["source.rs"])).fg, None);
    }

    #[test]
    fn syntax_tokens_never_wear_the_diff_colors() {
        for rule in rules() {
            assert!(!matches!(rule.style.fg, Some(Color::Green) | Some(Color::Red)));
        }
    }

    #[test]
    fn an_unknown_language_gets_no_colors() {
        let lines = vec!["@@ -1 +1 @@".to_string(), "+whatever".to_string()];
        assert!(highlight_diff("notes.unknownext", &lines).is_empty());
    }

    fn text_of(line: &StyledLine) -> String {
        line.iter().map(|(_, t)| t.as_str()).collect()
    }

    fn colored(line: &StyledLine, want: Color) -> Vec<&str> {
        line.iter().filter(|(s, _)| s.fg == Some(want)).map(|(_, t)| t.as_str()).collect()
    }

    #[test]
    fn the_two_sides_of_a_hunk_are_parsed_apart() {
        let lines: Vec<String> = [
            "diff --git a/f.rs b/f.rs",
            "@@ -1,3 +1,3 @@",
            " fn main() {",
            "-    let x = \"old\";",
            "+    let y = 42;",
            " }",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let hl = highlight_diff("f.rs", &lines);

        assert!(hl[0].is_empty(), "a file header carries no code");
        assert!(hl[1].is_empty(), "a hunk header carries no code");
        assert_eq!(text_of(&hl[2]), "fn main() {");
        assert_eq!(text_of(&hl[3]), "    let x = \"old\";");
        assert_eq!(text_of(&hl[4]), "    let y = 42;");

        assert!(colored(&hl[2], Color::Blue).contains(&"fn"), "{:?}", hl[2]);
        // The deleted line's string stays on the old side, the added line's
        // number on the new side: neither stream sees the other's text.
        assert!(colored(&hl[3], Color::Yellow).iter().any(|t| t.contains("old")), "{:?}", hl[3]);
        assert!(colored(&hl[4], Color::Magenta).contains(&"42"), "{:?}", hl[4]);
    }

    #[test]
    fn a_comment_on_one_side_does_not_color_the_other() {
        let lines: Vec<String> = [
            "@@ -1,2 +1,2 @@",
            "-// gone",
            "+let kept = 1;",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let hl = highlight_diff("f.rs", &lines);
        assert!(colored(&hl[1], Color::DarkGray).iter().any(|t| t.contains("gone")));
        assert!(colored(&hl[2], Color::DarkGray).is_empty(), "{:?}", hl[2]);
    }

    #[test]
    fn the_cache_returns_the_same_parse_twice() {
        let hl = Highlighter::default();
        let lines = vec!["@@ -1 +1 @@".to_string(), "+let x = 1;".to_string()];
        let a = hl.file("f.rs", &lines);
        let b = hl.file("f.rs", &lines);
        assert!(Rc::ptr_eq(&a, &b));
        let other = vec!["@@ -1 +1 @@".to_string(), "+let y = 2;".to_string()];
        assert!(!Rc::ptr_eq(&a, &hl.file("f.rs", &other)));
    }
}
