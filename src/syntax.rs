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

use crate::models::LineInfo;
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

/// syntect's own syntaxes plus the ones vendored in `assets/syntaxes`, baked
/// into the binary by the build script.
pub fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        syntect::dumps::from_binary(include_bytes!(concat!(env!("OUT_DIR"), "/syntaxes.bin")))
    })
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

/// Extensions no syntax names, and the one that reads the same language.
const ALIASES: [(&str, &str); 5] =
    [("jsx", "tsx"), ("mjs", "js"), ("cjs", "js"), ("hcl", "tf"), ("tfvars", "tf")];

fn syntax_for<'a>(set: &'a SyntaxSet, path: &str, first: Option<&str>) -> Option<&'a SyntaxReference> {
    let file = std::path::Path::new(path);
    let ext = file.extension().and_then(|e| e.to_str());
    let name = file.file_name().and_then(|e| e.to_str());
    let alias = ext.and_then(|e| ALIASES.iter().find(|(from, _)| *from == e).map(|(_, to)| *to));
    ext.and_then(|e| set.find_syntax_by_extension(e))
        .or_else(|| name.and_then(|n| set.find_syntax_by_extension(n)))
        .or_else(|| alias.and_then(|a| set.find_syntax_by_extension(a)))
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

impl Parsed {
    /// A whole file, to be parsed from its first line.
    fn for_blob(path: &str, text: &str) -> Self {
        let set = syntaxes();
        let syntax = syntax_for(set, path, text.lines().next());
        let start = syntax.unwrap_or_else(|| set.find_syntax_plain_text());
        Parsed {
            syntax,
            old: Stream::new(start),
            new: Stream::new(start),
            rows: Rc::new(Vec::new()),
            next: 0,
        }
    }

    /// Parse the file up to line `upto`, carrying the state across every line
    /// as an editor does: a string or a comment that opens on one line is still
    /// open on the next.
    fn extend_blob(&mut self, text: &str, upto: usize) {
        if self.syntax.is_none() || self.next >= upto {
            return;
        }
        let rows = Rc::make_mut(&mut self.rows);
        for line in text.lines().skip(self.next).take(upto - self.next) {
            let line = line.replace('\t', "    ");
            rows.push(match line.len() <= MAX_LINE {
                true => self.new.feed(&line),
                false => StyledLine::new(),
            });
            self.next += 1;
        }
    }
}

/// Color a whole file in one pass, for the callers that want all of it.
pub fn highlight_blob(path: &str, text: &str, upto: usize) -> Vec<StyledLine> {
    let mut parsed = Parsed::for_blob(path, text);
    parsed.extend_blob(text, upto);
    (*parsed.rows).clone()
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

/// Where a diff row's colors come from.
///
/// With both blobs of a file in hand the colors are the file's own, so a hunk
/// that opens inside a docstring reads like the rest of it. Without them each
/// hunk is parsed on its own, from its first line.
pub enum Painted {
    Hunks(Rc<Vec<StyledLine>>),
    Sides { old: Rc<Vec<StyledLine>>, new: Rc<Vec<StyledLine>> },
}

impl Painted {
    /// The colors of diff row `i`, which is line `info` of the two sides.
    pub fn row(&self, i: usize, info: Option<LineInfo>) -> Option<&StyledLine> {
        match self {
            Painted::Hunks(rows) => rows.get(i),
            Painted::Sides { old, new } => match info? {
                (_, Some(n)) => at(new, n),
                (Some(o), None) => at(old, o),
                (None, None) => None,
            },
        }
    }
}

/// Row `no` of a side, counting from one as a diff does.
fn at(rows: &[StyledLine], no: i64) -> Option<&StyledLine> {
    rows.get(usize::try_from(no).ok()?.checked_sub(1)?)
}

/// Whether the diff's lines are the blobs' lines, so a row can take its colors
/// from the file itself.
fn sides_agree(lines: &[String], info: &[LineInfo], old: Option<&String>, new: Option<&String>) -> bool {
    let split = |text: Option<&String>| {
        text.map(|t| t.lines().map(|l| l.replace('\t', "    ")).collect::<Vec<_>>())
    };
    let (old, new) = (split(old), split(new));
    let mut seen = false;
    for (i, line) in lines.iter().enumerate() {
        let Some(text) = content_of(line) else { continue };
        let Some(&(o, n)) = info.get(i) else { continue };
        for (side, no) in [(&old, o), (&new, n)] {
            // A side with no blob says nothing; one that has it must match.
            let (Some(side), Some(no)) = (side, no) else { continue };
            let Some(held) = usize::try_from(no).ok().and_then(|no| side.get(no.checked_sub(1)?)) else {
                return false;
            };
            if *held != text {
                return false;
            }
            seen = true;
        }
    }
    seen
}

type Cache = HashMap<(String, u64), Parsed>;
type Blobs = HashMap<String, String>;

/// Per-file highlight cache, so a redraw parses nothing and a first draw parses
/// only what the viewport shows.
///
/// The key holds the diff text's fingerprint, so a reloaded file is parsed
/// again and the two columns of a split diff each keep their own colors.
#[derive(Default)]
pub struct Highlighter {
    cache: RefCell<Cache>,
    blobs: RefCell<HashMap<String, Parsed>>,
    /// Whether a file's diff matches the blobs it names.
    agrees: RefCell<HashMap<(String, u64), bool>>,
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

    /// The colors of `path`, from its two blobs when the diff names them and
    /// `blobs` holds them, and from the hunks themselves otherwise.
    pub fn paint(
        &self,
        path: &str,
        lines: &[String],
        info: Option<&Vec<LineInfo>>,
        blobs: &Blobs,
        upto: usize,
    ) -> Painted {
        let hunks = || Painted::Hunks(self.rows(path, lines, upto));
        let (Some(info), Some((old_hash, new_hash))) = (info, crate::diff::blob_hashes(lines)) else {
            return hunks();
        };
        // Every side the diff shows needs its blob: painting one of them from
        // the file and leaving the other plain reads worse than parsing the
        // hunks. A worktree diff never has a blob for its new side, so it
        // keeps the hunks.
        let (old_text, new_text) = (blobs.get(&old_hash), blobs.get(&new_hash));
        let shows = |side: fn(&LineInfo) -> Option<i64>| info.iter().any(|i| side(i).is_some());
        if (shows(|i| i.0) && old_text.is_none()) || (shows(|i| i.1) && new_text.is_none()) {
            return hunks();
        }
        let key = (path.to_string(), fingerprint(lines));
        let mut agreed = self.agrees.borrow_mut();
        if agreed.len() >= MAX_CACHED && !agreed.contains_key(&key) {
            agreed.clear();
        }
        let agrees =
            *agreed.entry(key).or_insert_with(|| sides_agree(lines, info, old_text, new_text));
        drop(agreed);
        if !agrees {
            return hunks();
        }
        // The rows the viewport asks for name the file lines to parse.
        let bound = |side: fn(&LineInfo) -> Option<i64>| {
            info.iter().take(upto).filter_map(side).max().unwrap_or(0) as usize
        };
        Painted::Sides {
            old: self.blob_rows(path, &old_hash, old_text, bound(|i| i.0)),
            new: self.blob_rows(path, &new_hash, new_text, bound(|i| i.1)),
        }
    }

    fn blob_rows(&self, path: &str, hash: &str, text: Option<&String>, upto: usize) -> Rc<Vec<StyledLine>> {
        let Some(text) = text else { return Rc::new(Vec::new()) };
        let mut blobs = self.blobs.borrow_mut();
        if blobs.len() >= MAX_CACHED && !blobs.contains_key(hash) {
            blobs.clear();
        }
        let parsed = blobs.entry(hash.to_string()).or_insert_with(|| Parsed::for_blob(path, text));
        parsed.extend_blob(text, upto);
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
    fn typescript_and_tsx_read_like_the_editor() {
        let out = roles(
            "app.tsx",
            &[
                "import { useState } from \"react\";",
                "// a note",
                "export function Card<T>(props: Props<T>): JSX.Element {",
                "  const [open, setOpen] = useState<boolean>(false);",
                "  return <div className=\"card\" onClick={() => setOpen(!open)}>{`n=${props.id}`}</div>;",
            ],
        );
        assert_eq!(role_of(&out[0], "import"), Some(&Token::Import));
        assert!(held(&out[0], Token::Str).iter().any(|t| t.contains("react")));
        assert_eq!(role_of(&out[1], "// a note"), Some(&Token::Comment));
        assert_eq!(role_of(&out[2], "function"), Some(&Token::Keyword));
        assert_eq!(role_of(&out[2], "Card"), Some(&Token::FunctionDef));
        assert_eq!(role_of(&out[3], "const"), Some(&Token::Keyword));
        assert_eq!(role_of(&out[3], "false"), Some(&Token::Boolean));
        assert_eq!(role_of(&out[4], "return"), Some(&Token::Keyword));
        assert_eq!(role_of(&out[4], "div"), Some(&Token::Tag));
        assert_eq!(role_of(&out[4], "className"), Some(&Token::Property));
        assert!(held(&out[4], Token::Str).iter().any(|t| t.contains("card")));

        // `.ts` and `.jsx` answer too, and a plain `.ts` line still reads.
        let ts = roles("lib.ts", &["export const answer: number = 42;"]);
        assert_eq!(role_of(&ts[0], "42"), Some(&Token::Number));
        // A `.jsx` file reads with the TSX syntax, where a component is a type
        // and a plain element is a tag.
        let jsx = roles("app.jsx", &["const el = <Box title=\"hi\" />;"]);
        assert_eq!(role_of(&jsx[0], "Box"), Some(&Token::Type));
        assert_eq!(role_of(&jsx[0], "title"), Some(&Token::Property));
    }

    #[test]
    fn the_vendored_syntaxes_answer_for_their_files() {
        let set = syntaxes();
        for (ext, name) in [
            ("ts", "TypeScript"),
            ("tsx", "TypeScriptReact"),
            ("jsx", "TypeScriptReact"),
            ("mjs", "JavaScript"),
            ("toml", "TOML"),
            ("kt", "Kotlin"),
            ("swift", "Swift"),
            ("dart", "Dart"),
            ("graphql", "GraphQL"),
            ("tf", "Terraform"),
            ("hcl", "Terraform"),
            ("vue", "Vue Component"),
            ("ex", "Elixir"),
            ("zig", "Zig"),
            ("proto", "Protocol Buffer"),
            ("nix", "Nix"),
            ("fish", "Fish"),
        ] {
            let found = syntax_for(set, &format!("a.{ext}"), None).map(|s| s.name.as_str());
            assert_eq!(found, Some(name), "{ext}");
        }
        assert_eq!(syntax_for(set, "Dockerfile", None).map(|s| s.name.as_str()), Some("Dockerfile"));
    }

    const OLD_PY: &str = "import os\n\n\ndef alpha():\n    \"\"\"Docstring line 1\n    line 2\n    line 3\n    \"\"\"\n    return 1\n\n\ndef beta():\n    return 2\n";
    const NEW_PY: &str = "import sys\n\n\ndef alpha():\n    \"\"\"Docstring line 1\n    line 2 changed\n    line 3\n    \"\"\"\n    return 1\n\n\ndef beta():\n    return 2\n";

    /// A diff whose second hunk starts on the second line of a docstring.
    const DOCSTRING_DIFF: &str = "diff --git a/m.py b/m.py\n\
         index aaaaaaa..bbbbbbb 100644\n\
         --- a/m.py\n\
         +++ b/m.py\n\
         @@ -1,1 +1,1 @@\n\
         -import os\n\
         +import sys\n\
         @@ -6,4 +6,4 @@\n\
         -    line 2\n\
         +    line 2 changed\n\
         \x20    line 3\n\
         \x20    \"\"\"\n\
         \x20    return 1\n";

    fn docstring_case(blobs: Blobs) -> (Vec<String>, Vec<LineInfo>, Painted, usize) {
        let (files, infos) = crate::diff::parse_diff(DOCSTRING_DIFF);
        let (lines, info) = (files["m.py"].clone(), infos["m.py"].clone());
        let at = lines.iter().position(|l| l.contains("return 1")).unwrap();
        let hl = Highlighter::default();
        let painted = hl.paint("m.py", &lines, Some(&info), &blobs, lines.len());
        (lines, info, painted, at)
    }

    fn blobs_of(old: &str, new: &str) -> Blobs {
        [("aaaaaaa".to_string(), old.to_string()), ("bbbbbbb".to_string(), new.to_string())]
            .into_iter()
            .collect()
    }

    #[test]
    fn a_hunk_inside_a_docstring_reads_from_the_file() {
        let (_, info, painted, at) = docstring_case(blobs_of(OLD_PY, NEW_PY));
        let row = painted.row(at, Some(info[at])).expect("the line is colored");
        assert_eq!(role_of(row, "return"), Some(&Token::Keyword), "{row:?}");
        assert!(held(row, Token::Comment).is_empty(), "{row:?}");
        // The line the hunk opens on is still inside the docstring.
        let inside = info.iter().position(|&(_, n)| n == Some(6)).unwrap();
        let row = painted.row(inside, Some(info[inside])).unwrap();
        assert!(!held(row, Token::Comment).is_empty(), "{row:?}");
    }

    #[test]
    fn without_the_file_the_hunk_is_parsed_on_its_own() {
        // The closing quotes open a docstring of their own, and the rest of
        // the hunk falls inside it.
        let (_, info, painted, at) = docstring_case(Blobs::new());
        let row = painted.row(at, Some(info[at])).unwrap();
        assert_eq!(role_of(row, "return"), None, "{row:?}");
        assert!(!held(row, Token::Comment).is_empty(), "{row:?}");
    }

    #[test]
    fn a_blob_that_is_not_the_diff_is_not_used() {
        // A stale or wrong blob would paint the wrong lines, so the file falls
        // back to its hunks.
        let other = OLD_PY.replace("return 1", "return 99");
        let (_, info, painted, at) = docstring_case(blobs_of(&other, NEW_PY));
        assert!(matches!(painted, Painted::Hunks(_)));
        let row = painted.row(at, Some(info[at])).unwrap();
        assert!(!held(row, Token::Comment).is_empty(), "{row:?}");
    }

    #[test]
    fn a_side_the_diff_shows_needs_its_blob() {
        // Half the file would paint half the diff, so one missing blob sends
        // the file back to its hunks.
        let blobs: Blobs = [("bbbbbbb".to_string(), NEW_PY.to_string())].into_iter().collect();
        let (_, _, painted, _) = docstring_case(blobs);
        assert!(matches!(painted, Painted::Hunks(_)));
    }

    #[test]
    fn a_new_file_has_only_the_side_it_shows() {
        let raw = "diff --git a/n.py b/n.py\nnew file mode 100644\nindex 0000000..bbbbbbb\n\
                   --- /dev/null\n+++ b/n.py\n@@ -0,0 +1,2 @@\n+import os\n+x = 1\n";
        let (files, infos) = crate::diff::parse_diff(raw);
        let (lines, info) = (&files["n.py"], &infos["n.py"]);
        let blobs: Blobs =
            [("bbbbbbb".to_string(), "import os\nx = 1\n".to_string())].into_iter().collect();
        let painted = Highlighter::default().paint("n.py", lines, Some(info), &blobs, lines.len());
        assert!(matches!(painted, Painted::Sides { .. }));
        let at = lines.iter().position(|l| l.contains("import")).unwrap();
        let row = painted.row(at, Some(info[at])).unwrap();
        assert_eq!(role_of(row, "import"), Some(&Token::Import), "{row:?}");
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
