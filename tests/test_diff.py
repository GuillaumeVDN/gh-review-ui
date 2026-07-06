from ghreview.diff import parse_diff, compute_hunks

SAMPLE = """diff --git a/foo.py b/foo.py
index 111..222 100644
--- a/foo.py
+++ b/foo.py
@@ -1,3 +1,4 @@
 ctx
-old line
+new line
+added line
@@ -10,2 +11,2 @@
 keep
-gone
+fresh
diff --git a/bar.txt b/bar.txt
--- a/bar.txt
+++ b/bar.txt
@@ -0,0 +1,1 @@
+hello
"""


def test_parse_diff_splits_files():
    per_file, per_info = parse_diff(SAMPLE)
    assert set(per_file) == {"foo.py", "bar.txt"}
    # foo.py buffer starts at its own "diff --git" line
    assert per_file["foo.py"][0].startswith("diff --git a/foo.py")


def test_line_info_maps_added_and_deleted():
    _, per_info = parse_diff(SAMPLE)
    info = per_info["foo.py"]
    lines = parse_diff(SAMPLE)[0]["foo.py"]
    # find the "+new line" row and check its (old, new) mapping
    i = lines.index("+new line")
    old, new = info[i]
    assert old is None and new == 2  # first hunk new side starts at 1: ctx=1, new line=2
    j = lines.index("-old line")
    assert info[j] == (2, None)


def test_context_line_has_both_numbers():
    lines, info = parse_diff(SAMPLE)
    fl, fi = lines["foo.py"], info["foo.py"]
    i = fl.index(" ctx")
    assert fi[i] == (1, 1)


def test_compute_hunks_two_hunks():
    lines, _ = parse_diff(SAMPLE)
    hunks = compute_hunks(lines["foo.py"])
    assert len(hunks) == 2
    # each range starts on an "@@" line and is contiguous/ordered
    assert lines["foo.py"][hunks[0][0]].startswith("@@")
    assert hunks[0][1] == hunks[1][0]
    assert hunks[-1][1] == len(lines["foo.py"])


def test_empty_diff():
    assert parse_diff("") == ({}, {})
    assert compute_hunks([]) == []
