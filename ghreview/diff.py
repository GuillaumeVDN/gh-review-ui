"""Unified-diff parsing and hunk indexing (pure, no I/O)."""
import re

HUNK_RE = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def parse_diff(raw):
    """Split a `git diff` into per-file line lists and per-file line info.

    Returns ``(per_file_lines, per_file_line_info)`` where
    ``per_file_line_info[path]`` has one ``(old_line_no|None, new_line_no|None)``
    entry per diff line, letting callers map a diff row to a source line.
    """
    per_file = {}
    per_info = {}
    current_path = None
    buf = []
    info = []
    old_no = new_no = None
    in_hunk = False

    def flush():
        if current_path is not None:
            per_file[current_path] = buf[:]
            per_info[current_path] = info[:]

    for line in raw.splitlines():
        if line.startswith("diff --git "):
            flush()
            buf = [line]
            info = [(None, None)]
            try:
                current_path = line.split(" b/", 1)[1]
            except IndexError:
                current_path = None
            in_hunk = False
            old_no = new_no = None
            continue
        if current_path is None:
            continue
        buf.append(line)
        if line.startswith("@@"):
            m = HUNK_RE.match(line)
            if m:
                old_no = int(m.group(1))
                new_no = int(m.group(3))
                in_hunk = True
            info.append((None, None))
            continue
        if not in_hunk:
            info.append((None, None))
            continue
        if line.startswith("+") and not line.startswith("+++"):
            info.append((None, new_no))
            new_no += 1
        elif line.startswith("-") and not line.startswith("---"):
            info.append((old_no, None))
            old_no += 1
        elif line.startswith("\\"):  # "\ No newline at end of file"
            info.append((None, None))
        else:
            info.append((old_no, new_no))
            old_no += 1
            new_no += 1
    flush()
    return per_file, per_info


def compute_hunks(diff_lines):
    """Return ``[(start, end_exclusive), ...]`` for each contiguous change block.

    The app treats a "hunk" as a single diff block — a maximal run of added
    (``+``) / removed (``-``) lines. Context lines, ``@@`` headers and file
    headers break a run, so the extended context we render around changes never
    merges separate edits into one giant navigable region. Example::

        -test
        +test2
         context
        -test3
        +test4

    yields two blocks (``-test/+test2`` and ``-test3/+test4``).
    """
    blocks = []
    start = None
    for i, ln in enumerate(diff_lines):
        changed = ((ln.startswith("+") and not ln.startswith("+++")) or
                   (ln.startswith("-") and not ln.startswith("---")))
        if changed:
            if start is None:
                start = i
        elif start is not None:
            blocks.append((start, i))
            start = None
    if start is not None:
        blocks.append((start, len(diff_lines)))
    return blocks
