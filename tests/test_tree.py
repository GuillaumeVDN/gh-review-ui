from ghreview.models import State, FileEntry
from ghreview.tree import (
    build_tree, rebuild_tree, files_under_dir, fold_viewed_dirs, first_unviewed_index,
)


def make_state(paths_viewed):
    st = State()
    st.files = [FileEntry(path=p, viewed=v) for p, v in paths_viewed]
    rebuild_tree(st)
    return st


def test_build_tree_groups_dirs_and_sorts():
    files = [FileEntry("src/z.py", False), FileEntry("src/a.py", False),
             FileEntry("readme.md", False)]
    tree = build_tree(files, set())
    kinds = [(row[2], row[1]) for row in tree]
    # directory 'src' comes before the top-level file, dirs sort before files
    assert ("dir", "src") in kinds
    # within src, a.py precedes z.py
    names = [row[1] for row in tree if row[2] == "file"]
    assert names.index("a.py") < names.index("z.py")


def test_collapsed_dir_hides_children():
    files = [FileEntry("src/a.py", False), FileEntry("src/b.py", False)]
    full = build_tree(files, set())
    collapsed = build_tree(files, {"src"})
    assert len(collapsed) < len(full)
    assert all(row[2] != "file" for row in collapsed)


def test_files_under_dir():
    st = make_state([("src/a.py", False), ("src/b.py", True), ("docs/x.md", False)])
    got = {f.path for f in files_under_dir(st, "src")}
    assert got == {"src/a.py", "src/b.py"}


def test_fold_viewed_dirs_only_fully_viewed():
    st = make_state([("all/a.py", True), ("all/b.py", True),
                     ("mix/a.py", True), ("mix/b.py", False)])
    folded = fold_viewed_dirs(st)
    assert "all" in st.collapsed_dirs
    assert "mix" not in st.collapsed_dirs
    assert folded == 1


def test_first_unviewed_index():
    st = make_state([("a.py", True), ("b.py", False)])
    ti = first_unviewed_index(st)
    assert st.tree[ti][2] == "file"
    assert st.files[st.tree[ti][3]].path == "b.py"


def test_first_unviewed_none_when_all_viewed():
    st = make_state([("a.py", True), ("b.py", True)])
    assert first_unviewed_index(st) is None
