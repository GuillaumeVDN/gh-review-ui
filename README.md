# gh-review-ui

Minimal, lazygit-style terminal UI for reviewing GitHub PRs.

Five panes (left column stacked, right side full-height):
- **PRs** — open PRs, grouped into **My PRs** (you authored) and **Requested
  review** (you've been asked to review, or already reviewed and still open).
- **Commits** — commits of the active PR. All are selected by default (whole
  PR); unselect and pick a range (or a single commit) to review only those.
- **Files** — file tree of the currently checked-out PR, with viewed-state.
  When a commit range is selected, only the files it touches are listed.
- **Pending** — review comments queued locally, waiting to be submitted.
- **Right** — PR description + timeline when the PRs pane is focused,
  selected commit's message when the Commits pane is focused,
  diff of the highlighted / opened file otherwise. The current hunk is
  marked with a green side-bar.

Written in **Rust** with [ratatui](https://ratatui.rs) + crossterm. Backed by the
`gh` CLI (auth, PR list, diff) and `git` worktrees (checkout), plus GitHub's
GraphQL API for `viewedState` mutations and pending reviews.

## Requirements

- Rust (stable) + Cargo
- [`gh` CLI](https://cli.github.com/) — authenticated (`gh auth login`)
- `git`; and for the `e` editor shortcut, a running Neovim server + `hyprctl` (see below)
- A terminal that supports 256 colors and mouse events (foot, kitty, alacritty, wezterm, ghostty, xterm…)

## Install

```sh
git clone https://github.com/GuillaumeVDN/gh-review-ui.git
cd gh-review-ui
cargo build --release

# symlink the binary onto your PATH
ln -s "$PWD/target/release/gh-review-ui" ~/.local/bin/gh-review-ui
```

## Run

From any directory inside a GitHub repo checkout:

```sh
gh-review-ui        # or: cargo run --release
```

On start it will:
1. detect the repo from the current working directory (via `gh repo view`);
2. fetch open PRs where you're the author, a requested reviewer, or have already reviewed.

Opening a PR (`Enter` in the PRs pane) checks it out into its **own git
worktree** under `~/.cache/gh-review-ui/worktrees/<owner>__<repo>/pr-<n>` instead
of switching your main checkout's branch — so you (or agents) can keep working on
another branch while you review. The `e` editor shortcut opens the worktree copy
of the file, and worktrees are reused/refreshed on subsequent opens and on `r`.

## Keys

Global:
- `Tab` / `Shift-Tab` — cycle panes (PRs → Commits → Files → Pending edits → Pending comments → Diff)
- `0`…`5` — focus a pane directly (`0` Diff, `1` PRs, `2` Commits, `3` Files, `4` Pending edits, `5` Pending comments)
- `q` — quit
- `r` — refresh PR list + active PR (also reloads details when on the PRs pane)
- `Shift+J` / `Shift+K` — scroll one line: the PR summary when the PRs pane is
  focused, otherwise the diff (works from any pane)
- `c` — comment on the current hunk (opens the line picker)
- finish review — `Enter` in the Pending pane
- mouse wheel — scroll the pane under the cursor (stops at the content edge)
- click — focus a pane

PRs pane:
- `j` / `k` / arrows — move
- `Enter` — open the selected PR in a dedicated worktree (leaves your checkout untouched)
- `d` / `u` or PgDn / PgUp — scroll the details view
- `Shift+J` / `Shift+K` — scroll the PR summary line-by-line

The PR summary renders markdown (headings, lists, task-boxes, quotes, code
blocks, links), expands `<details>`/`<summary>` sections, and hides HTML
comments (`<!-- … -->`).

Commits pane:
- `j` / `k` — move
- `Space` — toggle the commit under the cursor
- `a` — select all / none
- `Enter` — apply the selection: reload the diff and file tree for the range
  spanning the earliest…latest selected commit (a gap between two selected
  commits is filled, so the checkboxes always match the reviewed range).
  Selecting every commit reviews the whole PR.

The right pane shows the selected commit's short SHA, author, date, and message.

Files pane:
- `j` / `k` — move (over files *and* folders)
- `Alt+j` / `Alt+k` — jump to the next / previous file, skipping folder rows
- `Space` — toggle viewed on file, or on all files under a folder
- `z` — fold every fully-viewed folder, then jump to the first unviewed file
- `e` — open the selected file in the editor (top of file)
- `Enter` — open file in the diff pane (folder: collapse / expand)

Pending edits pane (local, uncommitted changes in the worktree):
- `j` / `k` — move (`Alt+j` / `Alt+k` skip folder rows)
- `Space` — stage / unstage the file (or every file under a folder). The mark on
  the left is the pane's "viewed" equivalent: `[ ]` unstaged, `[~]` partly
  staged, `[✔]` fully staged (dimmed).
- `Enter` — show the file's local diff in the diff pane, with hunk navigation
- `c` — commit; what is staged is what gets committed. With an empty index the
  whole list is committed, as before.
- `P` — push the commits to the PR branch
- `d` — revert the file (worktree *and* index, back to the PR head)
- `e` — open the file in the editor

Pending comments pane:
- `j` / `k` — move
- `Enter` — open the submit-review modal
- `e` — edit the highlighted comment (reopens the editor, updates it on GitHub)
- `d` — discard the highlighted pending comment (deleted from the draft review on GitHub)

While the Pending pane is focused the right pane shows the selected comment's
target hunk (with the anchored line marked) and the comment body below it.

Diff pane:
- `j` / `k` / arrows — jump to next / previous change block
- `PgDn` / `PgUp` — page down / up
- `c` — start the comment line picker (see below)
- `e` — open the file in the editor at the current block's line
- `Esc` — back to the files pane
- on a **local** diff (opened with `Enter` from the pending-edits pane):
  - `Space` — stage / unstage the selected change block (lazygit-style)
  - `h` / `l` — move between the two columns of a partly-staged file

A partly-staged file splits the pane in two columns — unstaged on the left,
staged on the right — and the left panes shrink to make room. The focused column
(marked `▌`) is what `j`/`k` and `Space` act on: `Space` stages a block from the
left column and unstages one from the right.

A "hunk" here is a **change block** — a contiguous run of `+`/`-` lines. Context
(and the extra context rendered around edits) splits blocks, so two edits
separated by an unchanged line are two separate blocks you can navigate and
comment on independently. The focused block is highlighted with a cyan band plus
a green side-bar (only while the diff pane is focused).

### Commenting (`c` in diff pane)

`c` enters a **line picker** inside the current hunk:

- `j` / `k` — move the target line up / down (changed and context lines)
- `Shift+J` / `Shift+K` — extend a multi-line range from the anchor
- `Enter` — open the comment editor targeting that exact line / range
- `Esc` — cancel

The picked line (or range) is shown with a `▶` marker and reverse video. Then a
multi-line text editor opens:

- typing — insert text (long lines soft-wrap for display; no newline is added)
- `Shift+Enter` — insert a newline
- `Enter` — add to the pending review
- `Backspace` — delete character
- `Alt+Backspace` / `Ctrl+W` — delete the previous word
- Arrow keys / `Home` / `End` — move cursor
- `Esc` — cancel

Comments always attach to an actual changed line (or your picked line); a range
becomes a GitHub multi-line comment (`startLine`/`startSide` … `line`/`side`).

Adding a comment creates (or reuses) a **pending review on GitHub** and attaches
the comment to it, so pending comments persist across restarts and show up on
github.com's review UI. They stay private until you finish the review.

### Submit-review modal (`Enter` in the Pending pane)

A single modal with two halves:

1. **Description** editor (top). `Shift+Enter` inserts a newline; `Enter` moves
   focus down to the event choices; `Esc` cancels.
2. **Event** choice (bottom): `j`/`k` to pick *Comment*, *Request changes*, or
   *Approve*; `Enter` submits; `k` at the top jumps back to the description;
   `Esc` cancels.

On submit, the existing pending review (with all its comments) is submitted with
the chosen event.

> `Shift+Enter` / `Ctrl+Enter` / `Alt+Backspace` rely on the terminal's keyboard
> enhancement (kitty protocol) — foot, kitty, wezterm and ghostty qualify. The
> app requests it on start; on terminals without it, `Ctrl+W` also deletes a word.

### Editor integration (`e`)

`e` opens the file at the relevant line in a Neovim dedicated to the checkout
being reviewed (its own Ghostty window, grouped as a tab beside the TUI, talking
over `/tmp/nvim-ghr-<id>.sock`). Later opens reuse that Neovim and focus its
window. Closing the TUI closes the editors it started. This is wired for an
Omarchy/Hyprland + Neovim setup; adjust `open_in_dedicated_editor` in
`src/editor.rs` for a different editor or window manager.

## Notes

- "Viewed" state is stored server-side on GitHub; toggling here syncs to the PR review UI on github.com.
- Pending review comments are stored server-side too — close the app and they're still there when you return.
- Opening a PR fetches its head and (re)builds its worktree — press `r` to re-fetch and reload after new pushes.
- Review worktrees live under `~/.cache/gh-review-ui/worktrees/` and are reused across sessions; delete that directory (or `git worktree remove` them) to clean up.
- File pagination handles PRs with up to a few hundred files.
- Staging in the pending-edits pane writes to the real git index of the worktree
  (or of your checkout, when reviewing the locally checked-out PR).

## Project layout

The crate (`src/`) is split so almost all logic is UI-free and unit-tested:

| Module | Responsibility |
| --- | --- |
| `gh` | `gh` CLI / GraphQL and `git` subprocess wrappers |
| `api` | GitHub domain calls (PRs, files, diffs, reviews, worktrees) |
| `models` | data types + the central `State` |
| `diff` | unified-diff parsing and change-block indexing |
| `markdown` | markdown/HTML → styled terminal lines |
| `tree` | file-tree building and folding |
| `navigation` | cursor / hunk / selection logic over `State` |
| `theme` | ratatui `Style`s + diff/highlight helpers |
| `textbuffer` | modal text editor + soft-wrapping |
| `worker` | background thread running blocking `gh`/`git` jobs |
| `controller` | state transitions + job orchestration |
| `ui` | ratatui rendering of panes and overlays |
| `app` | terminal bootstrap, event loop, key/mouse dispatch |

## Development

```sh
cargo test        # unit tests
cargo run         # debug run
cargo build --release
```

The tests cover the pure logic — diff parsing / change blocks, markdown, the
file tree, hunk navigation, the text buffer + soft-wrap, theming, and the
layout/reveal-scroll math. The `gh` I/O and ratatui drawing are kept thin around
those tested seams.
