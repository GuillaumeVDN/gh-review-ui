"""Keyboard decoding: modifier+Enter escape sequences and flow control."""
import curses
import sys
import termios

# Synthetic key codes for modifier+Enter combos, decoded from terminal escape
# sequences (xterm modifyOtherKeys / kitty keyboard protocol). Chosen well above
# any real curses key value so they never collide.
KEY_SHIFT_ENTER = 1_000_001
KEY_CTRL_ENTER = 1_000_002

# Alt+<key> combos (terminals send ESC then the bare key byte).
KEY_ALT = {ord("j"): 1_000_010, ord("k"): 1_000_011}
KEY_ALT_J = KEY_ALT[ord("j")]
KEY_ALT_K = KEY_ALT[ord("k")]


def classify_seq(params, final):
    """Map a parsed CSI escape sequence to a synthetic key, or None.

    Handles both ``CSI 27;mod;code ~`` (modifyOtherKeys) and ``CSI code;mod u``
    (kitty) for Enter (code 13/10). ``mod`` is the xterm 1-based modifier mask.
    """
    nums = []
    for p in params.split(";"):
        try:
            nums.append(int(p))
        except ValueError:
            nums.append(-1)
    code = mod = None
    if final == "~" and len(nums) >= 3 and nums[0] == 27:
        mod, code = nums[1], nums[2]
    elif final == "u" and nums:
        code = nums[0]
        mod = nums[1] if len(nums) >= 2 else 1
    if code in (10, 13):
        m = (mod or 1) - 1
        if m & 1:            # shift
            return KEY_SHIFT_ENTER
        if m & 4:            # ctrl
            return KEY_CTRL_ENTER
        if m & 2:            # alt → treat as newline
            return KEY_SHIFT_ENTER
        return curses.KEY_ENTER
    return None


def get_key(win):
    """Like ``win.getch()`` but also decodes modifier+Enter escape sequences.

    Returns an int key code (including KEY_SHIFT_ENTER / KEY_CTRL_ENTER), -1 on
    timeout / unrecognised sequence, or 27 for a bare Escape.
    """
    ch = win.getch()
    if ch != 27:
        return ch
    win.timeout(15)
    try:
        c2 = win.getch()
        if c2 == -1:
            return 27  # bare Escape
        if c2 != ord("["):
            if c2 in KEY_ALT:  # Alt+<key>: ESC arrived glued to the key byte
                return KEY_ALT[c2]
            if c2 in (10, 13, curses.KEY_ENTER):
                # ESC-prefixed Enter (how some terminals send Shift/Alt+Enter):
                # treat as a newline, not Escape.
                return KEY_SHIFT_ENTER
            try:
                curses.ungetch(c2)
            except (curses.error, OverflowError):
                pass
            return 27
        params = []
        final = None
        for _ in range(12):
            c = win.getch()
            if c == -1:
                break
            if 0x40 <= c <= 0x7E:  # CSI final byte
                final = c
                break
            params.append(chr(c))
        if final is None:
            return 27
        tok = classify_seq("".join(params), chr(final))
        return tok if tok is not None else -1
    finally:
        win.timeout(80)


def disable_flow_control():
    """Turn off XON/XOFF so Ctrl+S / Ctrl+Q are ordinary keys, not flow control.

    Without this, pressing Ctrl+S freezes terminal output (XOFF) and the app
    appears to hang.
    """
    try:
        fd = sys.stdin.fileno()
        attrs = termios.tcgetattr(fd)
        attrs[0] &= ~(termios.IXON | termios.IXOFF | termios.IXANY)
        termios.tcsetattr(fd, termios.TCSANOW, attrs)
    except Exception:
        pass
