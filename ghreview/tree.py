"""File-tree building and folding for the Files pane (pure)."""


def build_tree(files, collapsed):
    """Turn a flat ``[FileEntry]`` into a display list of tree rows.

    Each row is ``(depth, name, kind, payload, is_collapsed)`` where ``kind`` is
    ``"dir"`` (payload = dir path) or ``"file"`` (payload = index into ``files``).
    """
    root = {}
    for i, f in enumerate(files):
        parts = f.path.split("/")
        node = root
        for p in parts[:-1]:
            nxt = node.get(p)
            if not isinstance(nxt, dict):
                nxt = {}
                node[p] = nxt
            node = nxt
        node[parts[-1]] = i

    display = []

    def walk(node, prefix, depth):
        entries = sorted(node.items(), key=lambda kv: (isinstance(kv[1], int), kv[0].lower()))
        for name, val in entries:
            if isinstance(val, dict):
                path = prefix + name
                is_collapsed = path in collapsed
                display.append((depth, name, "dir", path, is_collapsed))
                if not is_collapsed:
                    walk(val, path + "/", depth + 1)
            else:
                display.append((depth, name, "file", val, False))

    walk(root, "", 0)
    return display


def rebuild_tree(st):
    st.tree = build_tree(st.files, st.collapsed_dirs)
    if st.file_idx >= len(st.tree):
        st.file_idx = max(0, len(st.tree) - 1)


def files_under_dir(st, dir_path):
    prefix = dir_path + "/"
    return [f for f in st.files if f.path.startswith(prefix)]


def fold_viewed_dirs(st):
    """Collapse every folder whose files are all marked viewed. Returns count."""
    dirs = {}
    for f in st.files:
        parts = f.path.split("/")
        for i in range(len(parts) - 1):
            dirs.setdefault("/".join(parts[: i + 1]), []).append(f)
    folded = 0
    for path, dir_files in dirs.items():
        if dir_files and all(f.viewed for f in dir_files) and path not in st.collapsed_dirs:
            st.collapsed_dirs.add(path)
            folded += 1
    rebuild_tree(st)
    return folded


def first_unviewed_index(st):
    """Tree index of the first unviewed file, or None."""
    for ti, item in enumerate(st.tree):
        if item[2] == "file" and not st.files[item[3]].viewed:
            return ti
    return None
