//! Markdown / HTML → styled terminal lines (pure, no regex crate).

/// Style hint the renderer maps to an attribute.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Plain,
    Title,
    Meta,
    Sep,
    Dim,
    H1,
    H2,
    H3,
    Summary,
    Bullet,
    Quote,
    Code,
    Rule,
}

/// Private-use sentinel that isolates a `<summary>` on its own line.
const SUMMARY_MARK: char = '\u{E000}';

fn find_from(chars: &[char], start: usize, target: char) -> Option<usize> {
    (start..chars.len()).find(|&i| chars[i] == target)
}

/// `[text](url)` → `text (url)`, `![alt](url)` → `[image: alt]`.
fn convert_links(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let is_img = chars[i] == '!' && chars.get(i + 1) == Some(&'[');
        let is_link = chars[i] == '[';
        if is_img || is_link {
            let br_open = if is_img { i + 1 } else { i };
            if let Some(close) = find_from(&chars, br_open + 1, ']') {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(paren) = find_from(&chars, close + 2, ')') {
                        let text: String = chars[br_open + 1..close].iter().collect();
                        let url: String = chars[close + 2..paren].iter().collect();
                        if is_img {
                            out.push_str(&if text.is_empty() {
                                "[image]".to_string()
                            } else {
                                format!("[image: {text}]")
                            });
                        } else if text.is_empty() {
                            out.push_str(&url);
                        } else {
                            out.push_str(&format!("{text} ({url})"));
                        }
                        i = paren + 1;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Drop HTML tags (turning `<br>` into a space).
fn strip_tags(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(close) = find_from(&chars, i, '>') {
                let tag: String = chars[i + 1..close].iter().collect();
                if tag.trim_start_matches('/').to_lowercase().starts_with("br") {
                    out.push(' ');
                }
                i = close + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Flatten inline markdown/HTML to plain text for terminal display.
pub fn strip_inline_md(s: &str) -> String {
    let s = convert_links(s);
    let s = s.replace("**", "").replace("__", "").replace('`', "").replace('*', "");
    let s = strip_tags(&s);
    s.chars().filter(|c| !c.is_control()).collect()
}

fn remove_html_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("-->") {
            rest = &rest[start + end + 3..];
        } else {
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// Replace `<summary>…</summary>` (case-insensitive) with a newline-isolated
/// sentinel line so surrounding text isn't swallowed into the marker.
fn isolate_summaries(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::new();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find("<summary>") {
        let open = pos + rel;
        out.push_str(&text[pos..open]);
        let inner_start = open + "<summary>".len();
        if let Some(rel_end) = lower[inner_start..].find("</summary>") {
            let inner_end = inner_start + rel_end;
            let inner = text[inner_start..inner_end].trim();
            out.push('\n');
            out.push(SUMMARY_MARK);
            out.push_str(inner);
            out.push('\n');
            pos = inner_end + "</summary>".len();
        } else {
            out.push_str(&text[open..]);
            return out;
        }
    }
    out.push_str(&text[pos..]);
    out
}

fn is_rule(s: &str) -> bool {
    (s.len() >= 3) && (s.chars().all(|c| c == '-') || s.chars().all(|c| c == '*') || s.chars().all(|c| c == '_'))
}

/// Turn a markdown/HTML string into `(line, kind)` rows for the terminal.
pub fn markdown_lines(text: &str) -> Vec<(String, Kind)> {
    let text = remove_html_comments(text);
    let text = isolate_summaries(&text);
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in text.split('\n') {
        let stripped = raw.trim();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push((raw.to_string(), Kind::Code));
            continue;
        }
        if let Some(rest) = stripped.strip_prefix(SUMMARY_MARK) {
            out.push((format!("▸ {}", strip_inline_md(rest)), Kind::Summary));
            continue;
        }
        let low = stripped.to_lowercase();
        if low.starts_with("<details") || low.starts_with("</details") || low == "<summary>" {
            continue;
        }
        if stripped.is_empty() {
            out.push((String::new(), Kind::Plain));
            continue;
        }
        if is_rule(stripped) {
            out.push(("─".repeat(24), Kind::Rule));
            continue;
        }
        if stripped.starts_with('#') {
            let hashes = stripped.chars().take_while(|&c| c == '#').count();
            let body = stripped[hashes..].trim();
            let kind = match hashes {
                1 => Kind::H1,
                2 => Kind::H2,
                _ => Kind::H3,
            };
            out.push((strip_inline_md(body), kind));
            continue;
        }
        if let Some(rest) = stripped.strip_prefix('>') {
            out.push((format!("┃ {}", strip_inline_md(rest.trim())), Kind::Quote));
            continue;
        }
        if let Some((marker, body)) = list_item(stripped) {
            let body = checkbox(body);
            out.push((format!("{marker}{}", strip_inline_md(&body)), Kind::Bullet));
            continue;
        }
        out.push((strip_inline_md(raw), Kind::Plain));
    }
    out
}

/// Return `(marker, body)` for a bullet/ordered list item.
fn list_item(s: &str) -> Option<(String, &str)> {
    for p in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(p) {
            return Some(("• ".to_string(), rest));
        }
    }
    // ordered: "N. "
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let rest = &s[digits.len()..];
        if let Some(after) = rest.strip_prefix(". ") {
            return Some((format!("{digits}. "), after));
        }
    }
    None
}

fn checkbox(body: &str) -> String {
    if let Some(rest) = body.strip_prefix("[ ]") {
        format!("☐{rest}")
    } else if let Some(rest) = body.strip_prefix("[x]").or_else(|| body.strip_prefix("[X]")) {
        format!("☑{rest}")
    } else {
        body.to_string()
    }
}

fn hard_wrap_word(word: &str, width: usize, out: &mut Vec<String>, cur: &mut String) {
    let chars: Vec<char> = word.chars().collect();
    for chunk in chars.chunks(width) {
        let piece: String = chunk.iter().collect();
        if cur.is_empty() {
            *cur = piece;
        } else if cur.chars().count() + 1 + piece.chars().count() <= width {
            cur.push(' ');
            cur.push_str(&piece);
        } else {
            out.push(std::mem::take(cur));
            *cur = piece;
        }
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if word.chars().count() > width {
            hard_wrap_word(word, width, &mut lines, &mut cur);
        } else if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Word-wrap `(line, kind)` rows to `width`; code/rule/empty pass through.
pub fn wrap_styled(lines: Vec<(String, Kind)>, width: usize) -> Vec<(String, Kind)> {
    let width = width.max(4);
    let mut out = Vec::new();
    for (text, kind) in lines {
        if text.is_empty() || kind == Kind::Code || kind == Kind::Rule || text.chars().count() <= width {
            out.push((text, kind));
            continue;
        }
        for piece in wrap_text(&text, width) {
            out.push((piece, kind));
        }
    }
    out
}

fn jstr(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// Build `(line, kind)` rows for the PR summary from `gh pr view` JSON.
pub fn format_pr_details(data: &serde_json::Value) -> Vec<(String, Kind)> {
    let mut out = Vec::new();
    out.push((jstr(data, "title"), Kind::Title));
    let author = data
        .get("author")
        .and_then(|a| a.get("login"))
        .and_then(|l| l.as_str())
        .unwrap_or("?");
    let mut meta = format!("by {author} · {}", jstr(data, "state"));
    let decision = jstr(data, "reviewDecision");
    if !decision.is_empty() {
        meta.push_str(&format!(" · review: {decision}"));
    }
    let created = jstr(data, "createdAt");
    if created.len() >= 10 {
        meta.push_str(&format!(" · {}", &created[..10]));
    }
    out.push((meta, Kind::Meta));
    let url = jstr(data, "url");
    if !url.is_empty() {
        out.push((url, Kind::Meta));
    }
    out.push((String::new(), Kind::Plain));
    let body = jstr(data, "body");
    if body.trim().is_empty() {
        out.push(("(no description)".to_string(), Kind::Dim));
    } else {
        out.extend(markdown_lines(body.trim()));
    }
    out.push((String::new(), Kind::Plain));
    out.push(("━━━ Timeline ━━━".to_string(), Kind::Sep));

    let mut events: Vec<(bool, String, serde_json::Value)> = Vec::new();
    if let Some(arr) = data.get("comments").and_then(|c| c.as_array()) {
        for c in arr {
            events.push((false, jstr(c, "createdAt"), c.clone()));
        }
    }
    if let Some(arr) = data.get("reviews").and_then(|c| c.as_array()) {
        for r in arr {
            events.push((true, jstr(r, "createdAt"), r.clone()));
        }
    }
    events.sort_by(|a, b| a.1.cmp(&b.1));
    for (is_review, ts, item) in events {
        let who = item
            .get("author")
            .and_then(|a| a.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or("?");
        let when = ts.get(..19).unwrap_or(&ts).replace('T', " ");
        let head = if is_review {
            format!("[review] {who} · {} · {when}", jstr(&item, "state"))
        } else {
            format!("[comment] {who} · {when}")
        };
        out.push((head, Kind::Sep));
        let body = jstr(&item, "body");
        if body.trim().is_empty() {
            out.push(("(no body)".to_string(), Kind::Dim));
        } else {
            out.extend(markdown_lines(body.trim()));
        }
        out.push((String::new(), Kind::Plain));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_inline() {
        assert_eq!(strip_inline_md("a **b** c"), "a b c");
        assert_eq!(strip_inline_md("see [docs](http://x)"), "see docs (http://x)");
        assert_eq!(strip_inline_md("`code`"), "code");
        assert_eq!(strip_inline_md("![alt](u)"), "[image: alt]");
        assert_eq!(strip_inline_md("x<sub>y</sub>z"), "xyz");
    }

    #[test]
    fn comments_hidden() {
        let out = markdown_lines("before\n<!-- secret -->\nafter");
        let texts: Vec<&str> = out.iter().map(|(t, _)| t.as_str()).collect();
        assert!(texts.contains(&"before") && texts.contains(&"after"));
        assert!(out.iter().all(|(t, _)| !t.contains("secret")));
    }

    #[test]
    fn headings_lists_quotes_rules() {
        let out = markdown_lines("# H1\n## H2\n- item\n1. one\n> quoted\n---");
        assert!(out.contains(&("H1".to_string(), Kind::H1)));
        assert!(out.contains(&("H2".to_string(), Kind::H2)));
        assert!(out.contains(&("• item".to_string(), Kind::Bullet)));
        assert!(out.contains(&("1. one".to_string(), Kind::Bullet)));
        assert!(out.contains(&("┃ quoted".to_string(), Kind::Quote)));
        assert!(out.iter().any(|(_, k)| *k == Kind::Rule));
    }

    #[test]
    fn task_boxes_and_code() {
        let out = markdown_lines("- [ ] todo\n- [x] done\n```\n  keep **stars**\n```");
        assert!(out.contains(&("• ☐ todo".to_string(), Kind::Bullet)));
        assert!(out.contains(&("• ☑ done".to_string(), Kind::Bullet)));
        assert!(out.contains(&("  keep **stars**".to_string(), Kind::Code)));
    }

    #[test]
    fn details_summary() {
        let out = markdown_lines("before <summary>Click</summary> after");
        assert!(out.contains(&("▸ Click".to_string(), Kind::Summary)));
        assert!(out.iter().all(|(t, _)| !t.contains(SUMMARY_MARK)));
    }

    #[test]
    fn wrap_wraps_plain_not_code() {
        let input = vec![("x ".repeat(20), Kind::Plain), ("longcode".repeat(5), Kind::Code)];
        let w = wrap_styled(input, 20);
        assert!(w.iter().filter(|(_, k)| *k == Kind::Plain).count() > 1);
        assert_eq!(w.iter().filter(|(_, k)| *k == Kind::Code).count(), 1);
    }
}
