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
