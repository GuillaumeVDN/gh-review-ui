"""Markdown / HTML → styled terminal lines (pure).

``markdown_lines`` flattens a markdown string into ``(text, kind)`` tuples where
``kind`` is a style hint the renderer maps to an attribute. ``format_pr_details``
builds the PR-summary line list, and ``wrap_styled`` word-wraps those lines.
"""
import re
import textwrap

HTML_COMMENT_RE = re.compile(r"<!--.*?-->", re.DOTALL)
INLINE_LINK_RE = re.compile(r"!?\[([^\]]*)\]\(([^)]+)\)")
SUMMARY_RE = re.compile(r"<summary>(.*?)</summary>", re.IGNORECASE | re.DOTALL)
HEADING_RE = re.compile(r"(#{1,6})\s+(.*)")
RULE_RE = re.compile(r"^(-{3,}|\*{3,}|_{3,})$")
BULLET_RE = re.compile(r"^([-*+])\s+(.*)")
ORDERED_RE = re.compile(r"^(\d+)\.\s+(.*)")
TAG_RE = re.compile(r"<[^>]+>")
CTRL_RE = re.compile(r"[\x00-\x08\x0b-\x1f\x7f]")
SUMMARY_MARK = ""  # private-use sentinel, stripped before display


def strip_inline_md(s):
    """Flatten inline markdown/HTML to plain text for terminal display."""
    def link_sub(m):
        text, url = m.group(1), m.group(2)
        if m.group(0).startswith("!"):
            return f"[image: {text}]" if text else "[image]"
        return f"{text} ({url})" if text else url
    s = INLINE_LINK_RE.sub(link_sub, s)
    s = re.sub(r"\*\*(.+?)\*\*", r"\1", s)
    s = re.sub(r"__(.+?)__", r"\1", s)
    s = re.sub(r"(?<!\*)\*(?!\*)(.+?)\*(?!\*)", r"\1", s)
    s = re.sub(r"`([^`]+)`", r"\1", s)
    s = re.sub(r"(?i)<br\s*/?>", " ", s)
    s = TAG_RE.sub("", s)
    s = CTRL_RE.sub("", s)
    return s


def markdown_lines(text):
    """Return ``[(line, kind), ...]`` from a markdown/HTML string.

    HTML comments are stripped, ``<details>``/``<summary>`` are surfaced, and
    common inline markdown is flattened for terminal rendering.
    """
    text = HTML_COMMENT_RE.sub("", text or "")
    # Isolate <summary> onto its own line with a private-use sentinel so any
    # surrounding text on the same line is not swallowed into the marker.
    text = SUMMARY_RE.sub(lambda m: "\n" + SUMMARY_MARK + m.group(1).strip() + "\n", text)
    out = []
    in_code = False
    for raw in text.splitlines():
        stripped = raw.strip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            in_code = not in_code
            continue
        if in_code:
            out.append((raw, "code"))
            continue
        if stripped.startswith(SUMMARY_MARK):
            out.append(("▸ " + strip_inline_md(stripped[len(SUMMARY_MARK):]), "summary"))
            continue
        low = stripped.lower()
        if low.startswith("<details") or low.startswith("</details") or low == "<summary>":
            continue
        if not stripped:
            out.append(("", "plain"))
            continue
        if RULE_RE.match(stripped):
            out.append(("─" * 24, "rule"))
            continue
        m = HEADING_RE.match(stripped)
        if m:
            level = len(m.group(1))
            kind = "h1" if level == 1 else ("h2" if level == 2 else "h3")
            out.append((strip_inline_md(m.group(2)), kind))
            continue
        if stripped.startswith(">"):
            out.append(("┃ " + strip_inline_md(stripped.lstrip(">").strip()), "quote"))
            continue
        m = BULLET_RE.match(stripped) or ORDERED_RE.match(stripped)
        if m:
            marker = "• " if m.re is BULLET_RE else f"{m.group(1)}. "
            body = m.group(2)
            body = re.sub(r"^\[ \]\s*", "☐ ", body)
            body = re.sub(r"^\[[xX]\]\s*", "☑ ", body)
            out.append((marker + strip_inline_md(body), "bullet"))
            continue
        out.append((strip_inline_md(raw), "plain"))
    return out


def format_pr_details(data):
    """Build ``[(line, kind), ...]`` for the PR summary from `gh pr view` JSON."""
    out = []
    add = lambda s, k="plain": out.append((s, k))
    title = data.get("title") or ""
    state = data.get("state") or ""
    author = (data.get("author") or {}).get("login") or "?"
    decision = data.get("reviewDecision") or ""
    created = data.get("createdAt") or ""
    url = data.get("url") or ""
    add(title, "title")
    line = f"by {author} · {state}"
    if decision:
        line += f" · review: {decision}"
    if created:
        line += f" · {created[:10]}"
    add(line, "meta")
    if url:
        add(url, "meta")
    add("", "plain")
    body = (data.get("body") or "").strip()
    if body:
        out.extend(markdown_lines(body))
    else:
        add("(no description)", "dim")
    add("", "plain")
    add("━━━ Timeline ━━━", "sep")
    events = []
    for c in data.get("comments") or []:
        events.append(("comment", c.get("createdAt") or "", c))
    for r in data.get("reviews") or []:
        events.append(("review", r.get("createdAt") or "", r))
    events.sort(key=lambda e: e[1] or "")
    for kind, ts, item in events:
        who = (item.get("author") or {}).get("login") or "?"
        when = (ts or "")[:19].replace("T", " ")
        if kind == "review":
            head = f"[review] {who} · {item.get('state') or ''} · {when}"
        else:
            head = f"[comment] {who} · {when}"
        add(head, "sep")
        body = (item.get("body") or "").strip()
        if body:
            out.extend(markdown_lines(body))
        else:
            add("(no body)", "dim")
        add("", "plain")
    return out


def wrap_styled(lines, width):
    """Word-wrap ``(line, kind)`` tuples to ``width``; code/rule/empty pass through."""
    if width < 4:
        width = 4
    out = []
    for text, kind in lines:
        if not text or kind in ("code", "rule"):
            out.append((text, kind))
            continue
        if len(text) <= width:
            out.append((text, kind))
            continue
        for piece in textwrap.wrap(text, width, break_long_words=True,
                                   break_on_hyphens=False) or [""]:
            out.append((piece, kind))
    return out
