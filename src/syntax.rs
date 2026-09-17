//! Syntax highlighting of diff code, mapped onto the terminal palette.
//!
//! A hunk is a fragment of a file, and its two sides are two different texts.
//! Each file is parsed twice: an "old" stream of the context and deleted lines,
//! a "new" stream of the context and added lines. Both restart at every `@@`
//! header, because the lines between two hunks are missing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::models::LineInfo;
use crate::theme::{classify_diff_line, DiffKind, Token};

/// File contents by blob hash.
pub type Blobs = HashMap<String, Arc<str>>;

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

/// A diff parsed hunk by hunk, for a file whose blobs are out of reach.
struct HunkParse {
    syntax: Option<&'static SyntaxReference>,
    old: Stream,
    new: Stream,
    /// The first diff line no stream has seen yet.
    next: usize,
}

impl HunkParse {
    fn new(path: &str, lines: &[String]) -> Self {
        let set = syntaxes();
        let first = lines.iter().find_map(|l| content_of(l));
        let syntax = syntax_for(set, path, first.as_deref());
        let start = syntax.unwrap_or_else(|| set.find_syntax_plain_text());
        HunkParse { syntax, old: Stream::new(start), new: Stream::new(start), next: 0 }
    }

    /// The rows from where the last call stopped up to `upto`, or as many as
    /// `deadline` leaves room for.
    fn extend(&mut self, lines: &[String], upto: usize, deadline: Instant) -> Vec<StyledLine> {
        let mut out = Vec::new();
        let Some(syntax) = self.syntax else {
            while self.next < upto {
                out.push(StyledLine::new());
                self.next += 1;
            }
            return out;
        };
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
            out.push(match (kind, text) {
                (DiffKind::Add, Some(t)) => self.new.feed(&t),
                (DiffKind::Del, Some(t)) => self.old.feed(&t),
                (DiffKind::Context, Some(t)) => {
                    self.old.feed(&t);
                    self.new.feed(&t)
                }
                _ => StyledLine::new(),
            });
            self.next += 1;
            if Instant::now() >= deadline {
                break;
            }
        }
        out
    }
}

/// One whole file, parsed from its first line as an editor reads it.
struct FileParse {
    syntax: Option<&'static SyntaxReference>,
    stream: Stream,
    rows: Vec<StyledLine>,
    next: usize,
}

impl FileParse {
    fn new(path: &str, text: &str) -> Self {
        let set = syntaxes();
        let syntax = syntax_for(set, path, text.lines().next());
        let start = syntax.unwrap_or_else(|| set.find_syntax_plain_text());
        FileParse { syntax, stream: Stream::new(start), rows: Vec::new(), next: 0 }
    }

    fn extend(&mut self, text: &str, upto: usize, deadline: Instant) {
        if self.syntax.is_none() || self.next >= upto {
            return;
        }
        for line in text.lines().skip(self.next).take(upto - self.next) {
            let line = line.replace('\t', "    ");
            self.rows.push(match line.len() <= MAX_LINE {
                true => self.stream.feed(&line),
                false => StyledLine::new(),
            });
            self.next += 1;
            if Instant::now() >= deadline {
                return;
            }
        }
    }

    /// Row `no`, counting from one as a diff does.
    fn row(&self, no: i64) -> Option<&StyledLine> {
        self.rows.get(usize::try_from(no).ok()?.checked_sub(1)?)
    }
}

/// Color one file's diff in one pass, for the callers that want all of it.
pub fn highlight_diff(path: &str, lines: &[String]) -> Vec<StyledLine> {
    HunkParse::new(path, lines).extend(lines, lines.len(), far())
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

fn fingerprint(lines: &[String]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lines.len().hash(&mut h);
    for l in lines {
        l.hash(&mut h);
    }
    h.finish()
}

/// Whether the diff's lines are the blobs' lines, so a row can take its colors
/// from the file itself.
fn sides_agree(lines: &[String], info: &[LineInfo], old: Option<&str>, new: Option<&str>) -> bool {
    let split = |text: Option<&str>| {
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

/// A file view: its path and the fingerprint of the diff it shows.
pub type Key = (String, u64);

/// What one file view needs colored, and everything the work takes.
pub struct Request {
    pub key: Key,
    path: String,
    lines: Arc<Vec<String>>,
    info: Arc<Vec<LineInfo>>,
    old: Option<Arc<str>>,
    new: Option<Arc<str>>,
    upto: usize,
}

/// What the highlighter asks of its thread.
pub enum Ask {
    Paint(Request),
    /// The diffs on screen are gone: drop everything.
    Forget,
}

/// Rows `from..` of a file view, colored.
pub struct Painting {
    pub key: Key,
    pub from: usize,
    pub rows: Vec<StyledLine>,
}

/// How long one slice of work runs before it publishes what it has.
const SLICE: Duration = Duration::from_millis(16);
/// How long a file's own colors may take before its hunks answer instead.
const WHOLE_FILE_BUDGET: Duration = Duration::from_millis(750);
/// The measurement that says how fast a file parses.
const PROBE: Duration = Duration::from_millis(30);
const PROBE_LINES: usize = 24;
/// A file the diff barely reads into is worth no measurement.
const PROBE_FREE: usize = 64;

/// Where a file view's colors come from.
///
/// With both blobs of a file in hand the colors are the file's own, so a hunk
/// that opens inside a docstring reads like the rest of it. Without them each
/// hunk is parsed on its own, from its first line.
enum Mode {
    Hunks(HunkParse),
    Sides { old: FileParse, new: FileParse },
}

struct Job {
    mode: Mode,
    /// Rows already handed to the front end.
    rows: usize,
}

/// A file parsed as far as the diff needs it, and how long the rest will take.
///
/// One line of a generated YAML costs what a screenful of code costs, and the
/// hunk of such a file sits thousands of lines down. The first lines say which
/// kind of file this is, at the price of a glance.
fn probe(path: &str, text: &str, deepest: usize) -> (FileParse, bool) {
    let mut parse = FileParse::new(path, text);
    if deepest <= PROBE_FREE {
        return (parse, true);
    }
    // The first line of a language pays for its matchers, once per run.
    parse.extend(text, 1, far());
    let start = Instant::now();
    parse.extend(text, PROBE_LINES.min(deepest), start + PROBE);
    let read = parse.next.saturating_sub(1).max(1) as f64;
    let whole = start.elapsed().as_secs_f64() / read * deepest as f64;
    (parse, whole <= WHOLE_FILE_BUDGET.as_secs_f64())
}

impl Job {
    fn new(req: &Request) -> Job {
        let (old, new) = (req.old.as_deref(), req.new.as_deref());
        // Every side the diff shows needs its blob: painting one of them from
        // the file and leaving the other plain reads worse than parsing the
        // hunks. A worktree diff never has a blob for its new side.
        let shows = |side: fn(&LineInfo) -> Option<i64>| req.info.iter().any(|i| side(i).is_some());
        let deepest =
            |side: fn(&LineInfo) -> Option<i64>| req.info.iter().filter_map(side).max().unwrap_or(0) as usize;
        let named = (!shows(|i| i.0) || old.is_some()) && (!shows(|i| i.1) || new.is_some());
        let mode = match named && sides_agree(&req.lines, &req.info, old, new) {
            true => {
                let (old, old_ok) = probe(&req.path, old.unwrap_or_default(), deepest(|i| i.0));
                let (new, new_ok) = probe(&req.path, new.unwrap_or_default(), deepest(|i| i.1));
                match old_ok && new_ok {
                    true => Mode::Sides { old, new },
                    false => Mode::Hunks(HunkParse::new(&req.path, &req.lines)),
                }
            }
            false => Mode::Hunks(HunkParse::new(&req.path, &req.lines)),
        };
        Job { mode, rows: 0 }
    }
}

/// The work itself: it owns the parsers and hands back rows as they come.
#[derive(Default)]
pub struct Painter {
    jobs: HashMap<Key, Job>,
    order: Vec<Key>,
}

impl Painter {
    /// Color what `req` asks for, stopping at `deadline`. The answer is the
    /// next rows of the view, and whether the request is finished.
    pub fn work(&mut self, req: &Request, deadline: Instant) -> (Painting, bool) {
        if !self.jobs.contains_key(&req.key) {
            if self.order.len() >= MAX_CACHED {
                let oldest = self.order.remove(0);
                self.jobs.remove(&oldest);
            }
            self.order.push(req.key.clone());
            self.jobs.insert(req.key.clone(), Job::new(req));
        }
        let job = self.jobs.get_mut(&req.key).expect("just inserted");
        let upto = req.upto.min(req.lines.len());
        let from = job.rows;
        let mut out = Vec::new();
        while job.rows < upto {
            match &mut job.mode {
                Mode::Hunks(parse) => {
                    let tail = parse.extend(&req.lines, upto, deadline);
                    job.rows += tail.len();
                    out.extend(tail);
                }
                Mode::Sides { old, new } => {
                    let (o, n) = req.info.get(job.rows).copied().unwrap_or((None, None));
                    let side = match (o, n) {
                        (_, Some(no)) => Some((new, req.new.as_deref(), no)),
                        (Some(no), None) => Some((old, req.old.as_deref(), no)),
                        (None, None) => None,
                    };
                    let row = match side {
                        None => Some(StyledLine::new()),
                        Some((parse, text, no)) => {
                            let text = text.unwrap_or_default();
                            parse.extend(text, no.max(0) as usize, deadline);
                            parse.row(no).cloned().or_else(|| (parse.next >= no.max(0) as usize).then(StyledLine::new))
                        }
                    };
                    let Some(row) = row else { break }; // the file is not read that far yet
                    out.push(row);
                    job.rows += 1;
                }
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        (Painting { key: req.key.clone(), from, rows: out }, job.rows >= upto)
    }

    fn forget(&mut self) {
        self.jobs.clear();
        self.order.clear();
    }
}

/// The colors a file view has so far, indexed by diff row. A row nobody has
/// colored yet is absent, and the caller draws that line plain.
#[derive(Default)]
pub struct Painted(Arc<Vec<StyledLine>>);

impl Painted {
    pub fn row(&self, i: usize) -> Option<&StyledLine> {
        self.0.get(i)
    }
}

enum Engine {
    /// The app: a thread of its own, answering through the event loop.
    Thread(Sender<Ask>),
    /// Tests and one-off callers: the work runs where it is asked for.
    Inline(RefCell<Painter>),
}

#[derive(Default)]
struct Front {
    rows: HashMap<Key, Arc<Vec<StyledLine>>>,
    /// The highest row count asked for, so no range is asked for twice.
    asked: HashMap<Key, usize>,
}

/// The rendering side of highlighting: it holds what is colored and asks for
/// the rest. It never parses anything itself.
pub struct Highlighter {
    front: RefCell<Front>,
    engine: Engine,
}

impl Default for Highlighter {
    fn default() -> Highlighter {
        Highlighter { front: RefCell::default(), engine: Engine::Inline(RefCell::default()) }
    }
}

impl Highlighter {
    /// A highlighter whose work runs on a thread of its own and lands back in
    /// the event loop as [`crate::worker::Msg::Painted`].
    pub fn threaded(results: Sender<crate::worker::Msg>) -> Highlighter {
        let (tx, rx) = std::sync::mpsc::channel::<Ask>();
        std::thread::spawn(move || paint_loop(rx, results));
        Highlighter { front: RefCell::default(), engine: Engine::Thread(tx) }
    }

    /// A highlighter that sends its requests to `tx` and nowhere else.
    pub fn sending(tx: Sender<Ask>) -> Highlighter {
        Highlighter { front: RefCell::default(), engine: Engine::Thread(tx) }
    }

    /// The colors of `path` as far as they are known, and a request for the
    /// rows up to `upto` that are still missing.
    pub fn paint(
        &self,
        path: &str,
        lines: &[String],
        info: Option<&Vec<LineInfo>>,
        blobs: &Blobs,
        upto: usize,
    ) -> Painted {
        let key = (path.to_string(), fingerprint(lines));
        let upto = upto.min(lines.len());
        let mut front = self.front.borrow_mut();
        let held = front.rows.entry(key.clone()).or_default().len();
        let asked = front.asked.get(&key).copied().unwrap_or(0);
        let wanted = upto > held && upto > asked;
        if wanted {
            front.asked.insert(key.clone(), upto);
        }
        drop(front);
        if wanted {
            let (old, new) = crate::diff::blob_hashes(lines).unwrap_or_default();
            let request = Request {
                key: key.clone(),
                path: path.to_string(),
                lines: Arc::new(lines.to_vec()),
                info: Arc::new(info.cloned().unwrap_or_default()),
                old: blobs.get(&old).cloned(),
                new: blobs.get(&new).cloned(),
                upto,
            };
            match &self.engine {
                Engine::Thread(tx) => {
                    let _ = tx.send(Ask::Paint(request));
                }
                Engine::Inline(painter) => {
                    let (painting, _) = painter.borrow_mut().work(&request, far());
                    self.absorb(painting);
                }
            }
        }
        Painted(self.front.borrow().rows.get(&key).cloned().unwrap_or_default())
    }

    /// Take in rows the painter finished. A result for a view that is gone, or
    /// one that does not carry on from what is held, is dropped.
    pub fn absorb(&self, painting: Painting) {
        let mut front = self.front.borrow_mut();
        let Some(rows) = front.rows.get_mut(&painting.key) else { return };
        if painting.from != rows.len() {
            return;
        }
        Arc::make_mut(rows).extend(painting.rows);
    }

    /// Drop everything: the diffs it colored are gone.
    pub fn reset(&self) {
        let mut front = self.front.borrow_mut();
        front.rows.clear();
        front.asked.clear();
        drop(front);
        match &self.engine {
            Engine::Thread(tx) => {
                let _ = tx.send(Ask::Forget);
            }
            Engine::Inline(painter) => painter.borrow_mut().forget(),
        }
    }
}

/// The highlighter thread: newest request first, one slice at a time, and a
/// result after every slice so a long file fills in from the top.
fn paint_loop(rx: Receiver<Ask>, results: Sender<crate::worker::Msg>) {
    let mut painter = Painter::default();
    let mut queue: Vec<Request> = Vec::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(ask) => take(&mut queue, &mut painter, ask),
                Err(_) => return,
            }
        }
        while let Ok(ask) = rx.try_recv() {
            take(&mut queue, &mut painter, ask);
        }
        let Some(request) = queue.pop() else { continue };
        let (painting, done) = painter.work(&request, Instant::now() + SLICE);
        if !painting.rows.is_empty() && results.send(crate::worker::Msg::Painted(painting)).is_err() {
            return;
        }
        if !done {
            queue.push(request);
        }
    }
}

fn take(queue: &mut Vec<Request>, painter: &mut Painter, ask: Ask) {
    match ask {
        Ask::Paint(request) => {
            // One request per view, the latest one: it asks for the most rows.
            queue.retain(|held| held.key != request.key);
            queue.push(request);
        }
        Ask::Forget => {
            queue.clear();
            painter.forget();
        }
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

    fn painted(blobs: Blobs) -> (Vec<String>, Vec<LineInfo>, Painted, usize) {
        let (files, infos) = crate::diff::parse_diff(DOCSTRING_DIFF);
        let (lines, info) = (files["m.py"].clone(), infos["m.py"].clone());
        let at = lines.iter().position(|l| l.contains("return 1")).unwrap();
        let hl = Highlighter::default();
        let out = hl.paint("m.py", &lines, Some(&info), &blobs, lines.len());
        (lines, info, out, at)
    }

    fn blobs_of(old: &str, new: &str) -> Blobs {
        [("aaaaaaa".to_string(), Arc::from(old)), ("bbbbbbb".to_string(), Arc::from(new))]
            .into_iter()
            .collect()
    }

    #[test]
    fn a_hunk_inside_a_docstring_reads_from_the_file() {
        let (_, info, out, at) = painted(blobs_of(OLD_PY, NEW_PY));
        let row = out.row(at).expect("the line is colored");
        assert_eq!(role_of(row, "return"), Some(&Token::Keyword), "{row:?}");
        assert!(held(row, Token::Comment).is_empty(), "{row:?}");
        // The line the hunk opens on is still inside the docstring.
        let inside = info.iter().position(|&(_, n)| n == Some(6)).unwrap();
        let row = out.row(inside).unwrap();
        assert!(!held(row, Token::Comment).is_empty(), "{row:?}");
    }

    #[test]
    fn without_the_file_the_hunk_is_parsed_on_its_own() {
        // The closing quotes open a docstring of their own, and the rest of
        // the hunk falls inside it.
        let (_, _, out, at) = painted(Blobs::new());
        let row = out.row(at).unwrap();
        assert_eq!(role_of(row, "return"), None, "{row:?}");
        assert!(!held(row, Token::Comment).is_empty(), "{row:?}");
    }

    #[test]
    fn a_blob_that_is_not_the_diff_is_not_used() {
        // A stale or wrong blob would paint the wrong lines, so the file falls
        // back to its hunks.
        let other = OLD_PY.replace("return 1", "return 99");
        let (_, _, out, at) = painted(blobs_of(&other, NEW_PY));
        let row = out.row(at).unwrap();
        assert!(!held(row, Token::Comment).is_empty(), "{row:?}");
    }

    #[test]
    fn a_side_the_diff_shows_needs_its_blob() {
        // Half the file would paint half the diff, so one missing blob sends
        // the file back to its hunks.
        let blobs: Blobs = [("bbbbbbb".to_string(), Arc::from(NEW_PY))].into_iter().collect();
        let (_, _, out, at) = painted(blobs);
        let row = out.row(at).unwrap();
        assert!(!held(row, Token::Comment).is_empty(), "{row:?}");
    }

    #[test]
    fn a_new_file_has_only_the_side_it_shows() {
        let raw = "diff --git a/n.py b/n.py\nnew file mode 100644\nindex 0000000..bbbbbbb\n\
                   --- /dev/null\n+++ b/n.py\n@@ -0,0 +1,2 @@\n+import os\n+x = 1\n";
        let (files, infos) = crate::diff::parse_diff(raw);
        let (lines, info) = (&files["n.py"], &infos["n.py"]);
        let blobs: Blobs =
            [("bbbbbbb".to_string(), Arc::from("import os\nx = 1\n"))].into_iter().collect();
        let out = Highlighter::default().paint("n.py", lines, Some(info), &blobs, lines.len());
        let at = lines.iter().position(|l| l.contains("import")).unwrap();
        let row = out.row(at).unwrap();
        assert_eq!(role_of(row, "import"), Some(&Token::Import), "{row:?}");
    }

    fn request_channel() -> (Highlighter, Receiver<Ask>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Highlighter::sending(tx), rx)
    }

    #[test]
    fn one_request_covers_a_range_asked_for_twice() {
        let (files, infos) = crate::diff::parse_diff(DOCSTRING_DIFF);
        let (lines, info) = (&files["m.py"], &infos["m.py"]);
        let (hl, asks) = request_channel();

        let out = hl.paint("m.py", lines, Some(info), &Blobs::new(), 4);
        assert!(out.row(0).is_none(), "nothing is colored before the answer");
        hl.paint("m.py", lines, Some(info), &Blobs::new(), 4);
        hl.paint("m.py", lines, Some(info), &Blobs::new(), 2);
        let asked: Vec<usize> = asks.try_iter().map(|a| match a {
            Ask::Paint(r) => r.upto,
            Ask::Forget => 0,
        }).collect();
        assert_eq!(asked, vec![4], "the same range is not asked for twice");

        // Scrolling further asks for the rest, and never for less.
        hl.paint("m.py", lines, Some(info), &Blobs::new(), 9);
        let asked: Vec<usize> = asks.try_iter().map(|a| match a {
            Ask::Paint(r) => r.upto,
            Ask::Forget => 0,
        }).collect();
        assert_eq!(asked, vec![9]);
    }

    #[test]
    fn rows_arrive_in_order_and_only_once() {
        let (files, infos) = crate::diff::parse_diff(DOCSTRING_DIFF);
        let (lines, info) = (&files["m.py"], &infos["m.py"]);
        let (hl, _asks) = request_channel();
        let key = ("m.py".to_string(), fingerprint(lines));
        hl.paint("m.py", lines, Some(info), &Blobs::new(), 9);

        let row = |t: Token| vec![(t, "x".to_string())];
        hl.absorb(Painting { key: key.clone(), from: 0, rows: vec![row(Token::Keyword); 2] });
        let out = hl.paint("m.py", lines, Some(info), &Blobs::new(), 9);
        assert_eq!(out.row(1).map(Vec::len), Some(1));
        assert!(out.row(2).is_none(), "the rest has not arrived");

        // A partial result carries on from what is held.
        hl.absorb(Painting { key: key.clone(), from: 2, rows: vec![row(Token::Str)] });
        let out = hl.paint("m.py", lines, Some(info), &Blobs::new(), 9);
        assert_eq!(out.row(2).map(|r| r[0].0), Some(Token::Str));
        // One that does not is dropped, so the rows stay in step.
        hl.absorb(Painting { key: key.clone(), from: 0, rows: vec![row(Token::Number); 5] });
        hl.absorb(Painting { key: key.clone(), from: 8, rows: vec![row(Token::Number)] });
        let out = hl.paint("m.py", lines, Some(info), &Blobs::new(), 9);
        assert_eq!(out.row(0).map(|r| r[0].0), Some(Token::Keyword));
        assert!(out.row(3).is_none());
    }

    #[test]
    fn a_result_for_a_diff_that_is_gone_is_dropped() {
        let (files, infos) = crate::diff::parse_diff(DOCSTRING_DIFF);
        let (lines, info) = (&files["m.py"], &infos["m.py"]);
        let (hl, asks) = request_channel();
        let key = ("m.py".to_string(), fingerprint(lines));
        hl.paint("m.py", lines, Some(info), &Blobs::new(), 9);

        hl.reset();
        assert!(matches!(asks.try_iter().last(), Some(Ask::Forget)));
        hl.absorb(Painting { key, from: 0, rows: vec![vec![(Token::Keyword, "x".into())]] });
        let out = hl.paint("m.py", lines, Some(info), &Blobs::new(), 9);
        assert!(out.row(0).is_none(), "the answer belongs to a diff that is gone");
    }

    #[test]
    fn a_slice_stops_and_the_next_one_carries_on() {
        let (files, infos) = crate::diff::parse_diff(DOCSTRING_DIFF);
        let (lines, info) = (files["m.py"].clone(), infos["m.py"].clone());
        let request = Request {
            key: ("m.py".to_string(), 0),
            path: "m.py".to_string(),
            lines: Arc::new(lines.clone()),
            info: Arc::new(info),
            old: Some(Arc::from(OLD_PY)),
            new: Some(Arc::from(NEW_PY)),
            upto: lines.len(),
        };
        let mut painter = Painter::default();
        // A deadline already past leaves one slice of lines behind.
        let (first, done) = painter.work(&request, Instant::now());
        assert!(!done && first.from == 0 && !first.rows.is_empty());
        let (second, done) = painter.work(&request, far());
        assert!(done);
        assert_eq!(second.from, first.rows.len());
        assert_eq!(first.rows.len() + second.rows.len(), lines.len());
    }

    #[test]
    fn an_unknown_language_gets_no_roles() {
        let lines = vec!["@@ -1 +1 @@".to_string(), "+whatever".to_string()];
        let rows = highlight_diff("notes.unknownext", &lines);
        assert_eq!(rows.len(), lines.len());
        assert!(rows.iter().all(Vec::is_empty));
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
    fn a_reloaded_diff_is_colored_again() {
        let hl = Highlighter::default();
        let lines = vec!["@@ -1 +1 @@".to_string(), "+let x = 1;".to_string()];
        let a = hl.paint("f.rs", &lines, None, &Blobs::new(), lines.len());
        assert_eq!(role_of(a.row(1).unwrap(), "let"), Some(&Token::Keyword));
        let other = vec!["@@ -1 +1 @@".to_string(), "+let y = 2;".to_string()];
        let b = hl.paint("f.rs", &other, None, &Blobs::new(), other.len());
        assert_eq!(text_of(b.row(1).unwrap()), "let y = 2;");
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
        let whole = highlight_diff("f.rs", &lines);
        let near = hl.paint("f.rs", &lines, None, &Blobs::new(), 50);
        assert_eq!(near.row(49), Some(&whole[49]));
        assert!(near.row(50).is_none());
        let far = hl.paint("f.rs", &lines, None, &Blobs::new(), 200);
        assert_eq!(far.row(199), Some(&whole[199]));
        assert_eq!(far.row(10), Some(&whole[10]));
        assert!(far.row(200).is_none());
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
