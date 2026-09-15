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

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::theme::{classify_diff_line, DiffKind, Token};

/// One diff line's code, split into pieces and what each piece is. The `+`/`-`
/// marker is not part of it: the gutter and the background carry the diff
/// semantics. An empty list means "not parsed", and the caller draws the line
/// plain. The palette turns the roles into colors at render time, so a theme
/// switch costs nothing.
pub type StyledLine = Vec<(Token, String)>;

/// A line longer than this is drawn plain and is not parsed. The matchers run
/// on the whole line, and one long quoted string costs more than a screenful
/// of ordinary code.
const MAX_LINE: usize = 600;

/// Files kept in the cache before it is dropped and rebuilt on demand.
const MAX_CACHED: usize = 64;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

struct Rule {
    scope: Scope,
    token: Token,
}

/// Scope prefixes, most specific first, and what each one means.
fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |name: &str, token: Token| Rule {
            scope: Scope::new(name).expect("static scope name"),
            token,
        };
        vec![
            rule("comment", Token::Comment),
            rule("constant.character.escape", Token::StringEscape),
            rule("constant.other.placeholder", Token::StringEscape),
            rule("string", Token::Str),
            rule("constant.numeric", Token::Number),
            rule("constant.language", Token::Boolean),
            rule("keyword.control.import", Token::Import),
            rule("keyword.other.import", Token::Import),
            rule("keyword.control.at-rule.include", Token::Import),
            rule("keyword.operator", Token::KeywordOperator),
            rule("storage.type.primitive", Token::TypeBuiltin),
            rule("storage.type", Token::Keyword),
            rule("storage.modifier", Token::Keyword),
            rule("keyword", Token::Keyword),
            rule("entity.name.function", Token::FunctionDef),
            rule("variable.function", Token::FunctionCall),
            rule("support.function", Token::FunctionBuiltin),
            rule("entity.name.type", Token::Type),
            rule("entity.name.class", Token::Type),
            rule("support.class", Token::Type),
            rule("support.type", Token::TypeBuiltin),
            rule("variable.parameter", Token::Parameter),
            rule("variable.language", Token::VariableBuiltin),
            rule("variable.other.member", Token::Property),
            rule("variable.other.property", Token::Property),
            rule("meta.attribute", Token::Property),
            rule("entity.name.tag", Token::Tag),
            rule("entity.other.attribute-name", Token::Property),
            rule("punctuation", Token::Punctuation),
            rule("invalid", Token::Invalid),
            rule("constant", Token::Number),
        ]
    })
}

/// What a piece of code is, from the innermost scope a rule matches.
///
/// A quote or a `#` opens the string or the comment it belongs to, so it takes
/// that color rather than the punctuation color.
pub fn scope_token(stack: &ScopeStack, text: &str) -> Token {
    let mut punctuation = false;
    for scope in stack.scopes.iter().rev() {
        let Some(rule) = rules().iter().find(|r| r.scope.is_prefix_of(*scope)) else { continue };
        // `and` reads as a word, `+=` as a mark between two values.
        let worded = text.trim().starts_with(char::is_alphabetic);
        let token = match rule.token {
            Token::KeywordOperator if !worded => Token::Operator,
            found => found,
        };
        if token == Token::Punctuation {
            punctuation = true;
            continue;
        }
        if punctuation {
            return match token {
                Token::Comment | Token::Str => token,
                _ => Token::Punctuation,
            };
        }
        return token;
    }
    if punctuation {
        Token::Punctuation
    } else {
        Token::Plain
    }
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
                push(&mut out, &self.stack, &line[last..at]);
                last = at;
            }
            if self.stack.apply(&op).is_err() {
                return StyledLine::new();
            }
        }
        if last < end {
            push(&mut out, &self.stack, &line[last..end]);
        }
        out
    }
}

fn push(out: &mut StyledLine, stack: &ScopeStack, text: &str) {
    let token = scope_token(stack, text);
    match out.last_mut() {
        Some((t, held)) if *t == token => held.push_str(text),
        _ => out.push((token, text.to_string())),
    }
}

/// One file's diff, parsed as far as the screen has asked for.
struct Parsed {
    syntax: Option<&'static SyntaxReference>,
    old: Stream,
    new: Stream,
    rows: Rc<Vec<StyledLine>>,
    /// The first line no stream has seen yet.
    next: usize,
}

impl Parsed {
    fn new(path: &str, lines: &[String]) -> Self {
        let set = syntaxes();
        let first = lines.iter().find_map(|l| content_of(l));
        let syntax = syntax_for(set, path, first.as_deref());
        let start = syntax.unwrap_or_else(|| set.find_syntax_plain_text());
        Parsed {
            syntax,
            old: Stream::new(start),
            new: Stream::new(start),
            rows: Rc::new(Vec::new()),
            next: 0,
        }
    }

    /// Parse up to line `upto`, from wherever the last call stopped.
    fn extend(&mut self, lines: &[String], upto: usize) {
        let Some(syntax) = self.syntax else { return };
        let upto = upto.min(lines.len());
        if self.next >= upto {
            return;
        }
        let rows = Rc::make_mut(&mut self.rows);
        while self.next < upto {
            let line = &lines[self.next];
            let kind = classify_diff_line(line);
            if kind == DiffKind::Hunk {
                // The lines between two hunks are missing, so neither side can
                // carry its state across the gap.
                self.old = Stream::new(syntax);
                self.new = Stream::new(syntax);
            }
            let text = content_of(line).filter(|t| t.len() <= MAX_LINE);
            rows.push(match (kind, text) {
                (DiffKind::Add, Some(t)) => self.new.feed(&t),
                (DiffKind::Del, Some(t)) => self.old.feed(&t),
                (DiffKind::Context, Some(t)) => {
                    self.old.feed(&t);
                    self.new.feed(&t)
                }
                _ => StyledLine::new(),
            });
            self.next += 1;
        }
    }
}

/// Color one file's diff in one pass, for the callers that want all of it.
pub fn highlight_diff(path: &str, lines: &[String]) -> Vec<StyledLine> {
    let mut parsed = Parsed::new(path, lines);
    parsed.extend(lines, lines.len());
    (*parsed.rows).clone()
}

fn fingerprint(lines: &[String]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lines.len().hash(&mut h);
    for l in lines {
        l.hash(&mut h);
    }
    h.finish()
}

type Cache = HashMap<(String, u64), Parsed>;

/// Per-file highlight cache, so a redraw parses nothing and a first draw parses
/// only what the viewport shows.
///
/// The key holds the diff text's fingerprint, so a reloaded file is parsed
/// again and the two columns of a split diff each keep their own colors.
#[derive(Default)]
pub struct Highlighter {
    cache: RefCell<Cache>,
}

impl Highlighter {
    /// The colors of `path`, parsed as far as line `upto`. Rows past what has
    /// been parsed are absent, and the caller draws those lines plain.
    pub fn rows(&self, path: &str, lines: &[String], upto: usize) -> Rc<Vec<StyledLine>> {
        let key = (path.to_string(), fingerprint(lines));
        let mut cache = self.cache.borrow_mut();
        if cache.len() >= MAX_CACHED && !cache.contains_key(&key) {
            cache.clear();
        }
        let parsed = cache.entry(key).or_insert_with(|| Parsed::new(path, lines));
        parsed.extend(lines, upto);
        parsed.rows.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(path: &str, code: &[&str]) -> Vec<Vec<(Token, String)>> {
        let mut lines = vec!["@@ -1,9 +1,9 @@".to_string()];
        lines.extend(code.iter().map(|l| format!("+{l}")));
        highlight_diff(path, &lines).split_off(1)
    }

    fn role_of<'a>(line: &'a [(Token, String)], text: &str) -> Option<&'a Token> {
        line.iter().find(|(_, held)| held.trim() == text).map(|(token, _)| token)
    }

    #[test]
    fn python_reads_like_the_editor() {
        let out = roles(
            "a.py",
            &[
                "import os",
                "@decorator",
                "class Foo(Base):",
                "    def method(self, count: int = 3) -> str:",
                "        value = self.name + \"a\\nb\"  # note",
                "        if count is None and value in items:",
            ],
        );
        assert_eq!(role_of(&out[0], "import"), Some(&Token::Import));
        assert_eq!(role_of(&out[2], "class"), Some(&Token::Keyword));
        assert_eq!(role_of(&out[2], "Foo"), Some(&Token::Type));
        assert_eq!(role_of(&out[3], "def"), Some(&Token::Keyword));
        assert_eq!(role_of(&out[3], "method"), Some(&Token::FunctionDef));
        assert_eq!(role_of(&out[3], "count"), Some(&Token::Parameter));
        assert_eq!(role_of(&out[3], "int"), Some(&Token::TypeBuiltin));
        assert_eq!(role_of(&out[3], "3"), Some(&Token::Number));
        assert_eq!(role_of(&out[4], "self"), Some(&Token::VariableBuiltin));
        assert!(held(&out[4], Token::Str).iter().any(|t| t.contains('a')));
        assert_eq!(role_of(&out[4], "\\n"), Some(&Token::StringEscape));
        assert_eq!(role_of(&out[4], "# note"), Some(&Token::Comment));
        assert_eq!(role_of(&out[5], "is"), Some(&Token::KeywordOperator));
        assert_eq!(role_of(&out[5], "and"), Some(&Token::KeywordOperator));
        assert_eq!(role_of(&out[5], "None"), Some(&Token::Boolean));
        // The quotes belong to the string they open.
        assert!(held(&out[4], Token::Str).iter().any(|t| t.starts_with('"')), "{:?}", out[4]);
    }

    #[test]
    fn rust_reads_like_the_editor() {
        let out = roles(
            "a.rs",
            &[
                "use crate::theme;",
                "pub fn run(&self, name: &str) -> u32 {",
                "    let ok = true; // why",
                "    self.total += 1;",
            ],
        );
        assert_eq!(role_of(&out[1], "fn"), Some(&Token::Keyword));
        assert_eq!(role_of(&out[1], "run"), Some(&Token::FunctionDef));
        assert_eq!(role_of(&out[1], "name"), Some(&Token::Parameter));
        assert_eq!(role_of(&out[2], "let"), Some(&Token::Keyword));
        assert_eq!(role_of(&out[2], "true"), Some(&Token::Boolean));
        assert_eq!(role_of(&out[2], "// why"), Some(&Token::Comment));
        assert_eq!(role_of(&out[3], "self"), Some(&Token::VariableBuiltin));
        assert_eq!(role_of(&out[3], "+="), Some(&Token::Operator));
        assert_eq!(role_of(&out[3], "1"), Some(&Token::Number));
    }

    #[test]
    fn an_unknown_language_gets_no_roles() {
        let lines = vec!["@@ -1 +1 @@".to_string(), "+whatever".to_string()];
        assert!(highlight_diff("notes.unknownext", &lines).is_empty());
    }

    fn text_of(line: &StyledLine) -> String {
        line.iter().map(|(_, t)| t.as_str()).collect()
    }

    fn held(line: &StyledLine, want: Token) -> Vec<&str> {
        line.iter().filter(|(t, _)| *t == want).map(|(_, held)| held.as_str()).collect()
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

        assert!(held(&hl[2], Token::Keyword).contains(&"fn"), "{:?}", hl[2]);
        // The deleted line's string stays on the old side, the added line's
        // number on the new side: neither stream sees the other's text.
        assert!(held(&hl[3], Token::Str).iter().any(|t| t.contains("old")), "{:?}", hl[3]);
        assert!(held(&hl[4], Token::Number).contains(&"42"), "{:?}", hl[4]);
    }

    #[test]
    fn a_comment_on_one_side_does_not_color_the_other() {
        let lines: Vec<String> = ["@@ -1,2 +1,2 @@", "-// gone", "+let kept = 1;"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let hl = highlight_diff("f.rs", &lines);
        assert!(held(&hl[1], Token::Comment).iter().any(|t| t.contains("gone")));
        assert!(held(&hl[2], Token::Comment).is_empty(), "{:?}", hl[2]);
    }

    #[test]
    fn the_cache_returns_the_same_parse_twice() {
        let hl = Highlighter::default();
        let lines = vec!["@@ -1 +1 @@".to_string(), "+let x = 1;".to_string()];
        let a = hl.rows("f.rs", &lines, lines.len());
        let b = hl.rows("f.rs", &lines, lines.len());
        assert!(Rc::ptr_eq(&a, &b));
        let other = vec!["@@ -1 +1 @@".to_string(), "+let y = 2;".to_string()];
        assert!(!Rc::ptr_eq(&a, &hl.rows("f.rs", &other, other.len())));
    }

    fn rust_diff(n: usize) -> Vec<String> {
        let mut lines = vec!["diff --git a/f.rs b/f.rs".to_string(), "@@ -1,1 +1,1 @@".to_string()];
        for i in 0..n {
            lines.push(format!("+    let x{i} = \"value {i}\"; // note {i}"));
        }
        lines
    }

    #[test]
    fn a_parse_picks_up_where_it_stopped() {
        let lines = rust_diff(300);
        let hl = Highlighter::default();
        let near = hl.rows("f.rs", &lines, 50);
        assert_eq!(near.len(), 50);
        let far = hl.rows("f.rs", &lines, 200);
        assert_eq!(far.len(), 200);
        let whole = highlight_diff("f.rs", &lines);
        assert_eq!(&far[..], &whole[..200]);
        assert_eq!(&near[..], &whole[..50]);
    }

    #[test]
    fn a_very_long_line_is_left_plain() {
        let long = format!("+    query: '{}'", "select 1, ".repeat(200));
        let lines = vec!["@@ -1 +1 @@".to_string(), long, "+    name: short".to_string()];
        let hl = highlight_diff("f.yml", &lines);
        assert!(hl[1].is_empty(), "the long line is not parsed");
        assert!(!hl[2].is_empty(), "the next line still is");
    }
}
