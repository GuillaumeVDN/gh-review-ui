import curses

from ghreview.textbuffer import TextArea, wrap_textarea
from ghreview.keys import KEY_SHIFT_ENTER, KEY_CTRL_ENTER


def test_typing_inserts_text():
    ta = TextArea()
    for ch in "hi":
        assert ta.handle(ord(ch)) is None
    assert ta.text() == "hi"


def test_enter_confirms_shift_enter_newlines():
    ta = TextArea("ab")
    assert ta.handle(KEY_SHIFT_ENTER) is None
    assert ta.text() == "ab\n"
    assert ta.handle(curses.KEY_ENTER) == "enter"
    assert ta.handle(KEY_CTRL_ENTER) == "ctrl_enter"
    assert ta.handle(27) == "cancel"


def test_ctrl_s_x_fallbacks():
    ta = TextArea()
    assert ta.handle(19) == "enter"       # Ctrl+S
    assert ta.handle(24) == "ctrl_enter"  # Ctrl+X


def test_backspace_joins_lines():
    ta = TextArea("ab\ncd")
    ta.row, ta.col = 1, 0
    ta.handle(curses.KEY_BACKSPACE)
    assert ta.text() == "abcd"
    assert (ta.row, ta.col) == (0, 2)


def test_wrap_textarea_wraps_and_maps_cursor():
    ta = TextArea("abcdefghij")  # len 10
    ta.row, ta.col = 0, 10
    visual, vr, vc = wrap_textarea(ta, 4)
    assert [t for _, _, t in visual] == ["abcd", "efgh", "ij"]
    assert (vr, vc) == (2, 2)


def test_wrap_textarea_exact_multiple_has_trailing_row():
    ta = TextArea("abcdefgh")  # len 8, width 4
    ta.row, ta.col = 0, 8
    visual, vr, vc = wrap_textarea(ta, 4)
    assert [t for _, _, t in visual] == ["abcd", "efgh", ""]
    assert (vr, vc) == (2, 0)  # cursor on the fresh trailing row


def test_wrap_textarea_multiline_cursor():
    ta = TextArea("ab\ncdefg")
    ta.row, ta.col = 1, 5
    _, vr, vc = wrap_textarea(ta, 4)
    assert (vr, vc) == (2, 1)


def test_delete_word_removes_previous_word():
    from ghreview.keys import KEY_ALT_BACKSPACE
    ta = TextArea("hello world")   # cursor at end (col 11)
    ta.handle(KEY_ALT_BACKSPACE)
    assert ta.text() == "hello " and ta.col == 6
    ta.handle(KEY_ALT_BACKSPACE)
    assert ta.text() == "" and ta.col == 0


def test_delete_word_eats_trailing_space():
    from ghreview.keys import KEY_ALT_BACKSPACE
    ta = TextArea("foo bar   ")   # trailing spaces then cursor at end
    ta.handle(KEY_ALT_BACKSPACE)
    assert ta.text() == "foo "     # spaces + "bar" removed


def test_delete_word_ctrl_w_alias():
    ta = TextArea("alpha beta")
    ta.handle(23)                  # Ctrl+W
    assert ta.text() == "alpha "


def test_delete_word_at_line_start_joins_previous():
    from ghreview.keys import KEY_ALT_BACKSPACE
    ta = TextArea("ab\ncd")
    ta.row, ta.col = 1, 0
    ta.handle(KEY_ALT_BACKSPACE)
    assert ta.text() == "abcd" and (ta.row, ta.col) == (0, 2)


def test_delete_word_mid_line():
    from ghreview.keys import KEY_ALT_BACKSPACE
    ta = TextArea("one two three")
    ta.row, ta.col = 0, 8          # cursor at the start of "three" (after "two ")
    ta.handle(KEY_ALT_BACKSPACE)
    assert ta.text() == "one three" and ta.col == 4
