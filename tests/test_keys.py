import curses

from ghreview.keys import classify_seq, KEY_SHIFT_ENTER, KEY_CTRL_ENTER


def test_modify_other_keys_shift_enter():
    assert classify_seq("27;2;13", "~") == KEY_SHIFT_ENTER


def test_modify_other_keys_ctrl_enter():
    assert classify_seq("27;5;13", "~") == KEY_CTRL_ENTER


def test_alt_enter_treated_as_newline():
    assert classify_seq("27;3;13", "~") == KEY_SHIFT_ENTER


def test_plain_enter_with_no_modifier():
    assert classify_seq("27;1;13", "~") == curses.KEY_ENTER


def test_kitty_u_form():
    assert classify_seq("13;2", "u") == KEY_SHIFT_ENTER
    assert classify_seq("13;5", "u") == KEY_CTRL_ENTER


def test_non_enter_sequences_ignored():
    assert classify_seq("1;2", "A") is None   # arrow-ish
    assert classify_seq("27;2;9", "~") is None  # shift+tab, not enter


class FakeWin:
    """Feeds a scripted sequence of getch() return values."""
    def __init__(self, seq):
        self.seq = list(seq)

    def getch(self):
        return self.seq.pop(0) if self.seq else -1

    def timeout(self, _):
        pass


def test_alt_j_decoded():
    from ghreview.keys import get_key, KEY_ALT_J, KEY_ALT_K
    assert get_key(FakeWin([27, ord("j")])) == KEY_ALT_J
    assert get_key(FakeWin([27, ord("k")])) == KEY_ALT_K


def test_bare_escape_then_timeout():
    from ghreview.keys import get_key
    assert get_key(FakeWin([27, -1])) == 27


def test_esc_prefixed_enter_is_newline():
    # Some terminals send Shift/Alt+Enter as ESC then CR/LF — must be a newline,
    # not Escape (which would cancel the modal).
    from ghreview.keys import get_key, KEY_SHIFT_ENTER
    assert get_key(FakeWin([27, 13])) == KEY_SHIFT_ENTER
    assert get_key(FakeWin([27, 10])) == KEY_SHIFT_ENTER
