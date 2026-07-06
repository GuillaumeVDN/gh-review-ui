"""External-editor integration (open a file at a line in a running nvim)."""
import os
import shlex
import subprocess

from .navigation import cur_file_path, current_hunk_editor_line


def open_in_editor(abs_path, line):
    """Fire-and-forget: open ``abs_path`` in the running nvim server at ``line``.

    Wired for an Omarchy/Hyprland + Neovim setup (nvim listening on
    ``/tmp/nvim.sock``); tweak this for a different editor / window manager.
    """
    script = (
        f"nvim --server /tmp/nvim.sock --remote {shlex.quote(abs_path)} "
        f'&& nvim --server /tmp/nvim.sock --remote-send ":{int(line)}<CR>" '
        f"&& (hyprctl dispatch focuswindow class:org.omarchy.nvim | grep -q ok "
        f"|| hyprctl dispatch focuswindow title:^n$)"
    )
    subprocess.Popen(
        ["/usr/bin/bash", "-c", script],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL, start_new_session=True,
    )


def open_current_in_editor(st, top=False):
    """Open the selected file — at the top, or at the current hunk's line."""
    path = cur_file_path(st)
    if not path:
        st.status = "No file selected."
        return
    line = 1 if top else current_hunk_editor_line(st, path)
    abs_path = os.path.join(st.repo_root or os.getcwd(), path)
    try:
        open_in_editor(abs_path, line)
        st.status = f"Opening {path}:{line} in editor…"
    except Exception as e:
        st.status = f"editor error: {type(e).__name__}: {e}"
