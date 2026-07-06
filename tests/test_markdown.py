from ghreview.markdown import (
    strip_inline_md, markdown_lines, wrap_styled, format_pr_details, SUMMARY_MARK,
)


def kinds(pairs):
    return [k for _, k in pairs]


def texts(pairs):
    return [t for t, _ in pairs]


def test_strip_inline_removes_emphasis_and_links():
    assert strip_inline_md("a **b** c") == "a b c"
    assert strip_inline_md("see [docs](http://x)") == "see docs (http://x)"
    assert strip_inline_md("`code`") == "code"
    assert strip_inline_md("![alt](u)") == "[image: alt]"


def test_strip_inline_removes_control_chars_and_tags():
    assert "\x00" not in strip_inline_md("a\x00b")
    assert strip_inline_md("x<sub>y</sub>z") == "xyz"


def test_html_comments_are_hidden():
    out = markdown_lines("before\n<!-- secret -->\nafter")
    assert "before" in texts(out) and "after" in texts(out)
    assert all("secret" not in t for t in texts(out))


def test_headings_lists_quotes_rules():
    out = markdown_lines("# H1\n## H2\n- item\n1. one\n> quoted\n---")
    d = dict(out)  # text -> kind (unique here)
    assert d["H1"] == "h1"
    assert d["H2"] == "h2"
    assert ("• item", "bullet") in out
    assert ("1. one", "bullet") in out
    assert ("┃ quoted", "quote") in out
    assert any(k == "rule" for _, k in out)


def test_task_list_checkboxes():
    out = markdown_lines("- [ ] todo\n- [x] done")
    assert ("• ☐ todo", "bullet") in out
    assert ("• ☑ done", "bullet") in out


def test_code_fence_preserved_verbatim():
    out = markdown_lines("```\n  keep  **stars**\n```")
    assert ("  keep  **stars**", "code") in out  # not flattened


def test_details_summary_surfaced_and_isolated():
    out = markdown_lines("before <summary>Click</summary> after")
    assert ("▸ Click", "summary") in out
    # surrounding text preserved on its own line, sentinel never leaks
    assert all(SUMMARY_MARK not in t for t in texts(out))
    assert "before" in "".join(texts(out)) and "after" in "".join(texts(out))


def test_wrap_styled_wraps_long_plain_but_not_code():
    wrapped = wrap_styled([("x " * 20, "plain"), ("longcodeline" * 5, "code")], 20)
    assert sum(1 for _, k in wrapped if k == "plain") > 1
    assert sum(1 for _, k in wrapped if k == "code") == 1  # code passes through


def test_format_pr_details_header_and_body():
    data = {"title": "My PR", "state": "OPEN", "author": {"login": "me"},
            "body": "## Summary\nhello", "comments": [], "reviews": []}
    out = format_pr_details(data)
    assert out[0] == ("My PR", "title")
    assert any(k == "sep" and "Timeline" in t for t, k in out)
    assert ("Summary", "h2") in out
