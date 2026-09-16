# gh-review-ui

Minimal, lazygit-style terminal UI for reviewing GitHub PRs.

Five panes (left column stacked, right side full-height):
- **PRs** — open PRs, grouped into **My PRs** (you authored) and **Requested
  review** (you've been asked to review, or already reviewed and still open).
- **Commits** — commits of the active PR. All are selected by default (whole
  PR); unselect and pick a range (or a single commit) to review only those.
- **Files** — file tree of the currently checked-out PR, with viewed-state.
  The diff decides the list: it holds the files only the checkout has, from
  commits GitHub has not seen, and leaves out the files the PR lists that the
  branch no longer changes. When a commit range is selected, only the files it
  touches are listed.
- **Pending** — review comments queued locally, waiting to be submitted.
- **Right** — PR description + timeline when the PRs pane is focused,
  selected commit's message when the Commits pane is focused,
  diff of the highlighted / opened file otherwise. The current stop — a change
  block, a pending comment or a review thread — is marked with a green side-bar.

Written in **Rust** with [ratatui](https://ratatui.rs) + crossterm. Backed by the
`gh` CLI (auth, PR list, diff) and `git` worktrees (checkout), plus GitHub's
GraphQL API for `viewedState` mutations, pending reviews and review threads.

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

Commits pane. On the branch the PR's head names, the list is the checkout's
own (`git log` since the branch left its base), so commits you have not pushed
are in it, in the same orange as the local edits. For a PR you are only looking
at, it is what GitHub has.

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
- `Space` — toggle viewed on file, or on all files under a folder. A mark does
  what `z` does next: it folds every fully-viewed folder and jumps to the first
  unviewed file.
- `z` — fold every fully-viewed folder, then jump to the first unviewed file
- `e` — open the selected file in the editor (top of file)
- `s` — switch the review diff between inline and side by side
- `Enter` — open file in the diff pane (folder: collapse / expand)

Pending edits pane (local, uncommitted changes in the worktree). `--edits`
lands here when there is something uncommitted, and on the Files pane when the
tree is clean:
- `j` / `k` — move (`Alt+j` / `Alt+k` skip folder rows)
- `Space` — stage / unstage the file (or every file under a folder). The mark on
  the left is the pane's "viewed" equivalent: `[ ]` unstaged, `[~]` partly
  staged, `[✔]` fully staged (dimmed). Staging does what `z` does next: it folds
  every fully-staged folder and jumps to the first unstaged file.
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
- `j` / `k` / arrows — jump to next / previous **stop**: the change blocks and
  the comments drawn inline, in the order they read
- `PgDn` / `PgUp` — page down / up
- `c` — start the comment line picker on the enclosing block (see below)
- `s` — switch the review diff between inline and side by side
- `e` — open the file in the editor at the current block's line
- `Esc` — back to the files pane
- on a **pending comment** of yours:
  - `Enter` / `e` — edit it (the same editor the Pending pane opens)
  - `d` — discard it, from the draft review on GitHub too
- on an **unresolved review thread** (read-only):
  - `Enter` — answer it: a new comment of yours, on the same line
  - `o` — open the thread on github.com
- on a **local** diff (opened with `Enter` from the pending-edits pane):
  - `Space` — stage / unstage the selected change block (lazygit-style)
  - `h` / `l` — move between the two columns of a partly-staged file

Every diff carries a line-number gutter, dimmed on the left of the code: the old
number then the new one inline, and one number per column side by side. A pane
too narrow to hold both the numbers and the code drops the gutter.

### Colors

The app wears the active [Omarchy](https://omarchy.org) theme, so a diff reads
like the Neovim next to it: comments in `dark_foreground` italic, strings in
`green`, keywords in `magenta` bold, types in `yellow` bold, `self` in `red`
italic. It reads `~/.local/state/omarchy/current/theme/colors.toml`
(`$XDG_STATE_HOME` when that is set), and follows a theme switch within a
second, with no restart.

The code itself carries syntax colors. The language comes from the file
extension, then from the first line; a file neither answers for stays plain.
Each file is parsed twice, once per side of the diff, so a deleted line and the
line that replaces it each read as their own text.

The syntaxes are [syntect](https://github.com/trishume/syntect)'s own set plus
the ones under `assets/syntaxes`, which cover TypeScript, TSX, JSX, TOML,
Dockerfile, Kotlin, Swift, Dart, GraphQL, Terraform, Vue, Elixir, Zig,
Protobuf, Nix, fish, nginx and `.env`. To add one, drop a Sublime Text
`.sublime-syntax` file (the v1 format, not a `version: 2` one) and its upstream
LICENSE in `assets/syntaxes/<Name>/` and build: `build.rs` bakes the whole set
into the binary, and skips with a warning any file syntect cannot read.

The diff meaning lives in the gutter and the background. The `+` / `-` marker
and the line numbers wear the theme's green and red, and the changed line sits
on that same hue blended into the background: about a fifth of it for a changed
line, a third for the focused change block, next to the `▌` side-bar. A theme
of low contrast of its own gets a fainter tint, so the code on it keeps reading.

A file is colored as far down as you have scrolled, and a line over 600
characters stays plain: the matchers run on the whole line, and one long quoted
string costs more than a screenful of ordinary code.

Without an Omarchy theme the app keeps its own ANSI colors, in a dark and a
light set:

- `GH_REVIEW_UI_THEME=dark` or `GH_REVIEW_UI_THEME=light` picks one, and names
  the mode of an Omarchy theme too;
- else the app asks the terminal for its own background color (OSC 11) and
  reads the luminance of the answer. The terminal has 150 ms to answer, and the
  question is asked before the first key is read;
- else it reads `COLORFGBG` (`fg;bg`), which some terminals export;
- else it takes the dark set.

`s` draws the review diff side by side: the old side on the left, the new side
on the right, and the left panes shrink to make room. Each deletion sits across
from the addition that replaces it; an unpaired change leaves the other column
empty. File headers, `@@` headers and inline comments span both columns, and
local worktree edits stay on the new side. Everything else carries over from the
inline view: the change-block band, the comment picker, `j`/`k`, and the scroll
keeps its place across the switch. A local diff from the pending-edits pane
always stays inline, since its columns are the index instead.

A partly-staged file splits the pane in two columns — unstaged on the left,
staged on the right — and the left panes shrink to make room. The focused column
(marked `▌`) is what `j`/`k` and `Space` act on: `Space` stages a block from the
left column and unstages one from the right.

A "hunk" here is a **change block** — a contiguous run of `+`/`-` lines. Context
(and the extra context rendered around edits) splits blocks, so two edits
separated by an unchanged line are two separate blocks you can navigate and
comment on independently. The focused block is highlighted with a cyan band plus
a green side-bar (only while the diff pane is focused).

### Stops, and the threads under the code

`j`/`k` in the diff pane walk the **stops**: the change blocks plus every
comment drawn under a line. A comment you stop on wears the same side-bar and
the same focused band as a block, so what the next key acts on is always the
thing that looks selected. `c` still picks a line in the enclosing block.

Under their anchored line the diff shows:

- your own **pending comments**, in cyan, which `Enter`/`e` edits and `d`
  discards;
- every **unresolved review thread** of the PR, from any author, read-only, on
  a faint band of the theme's selection color: a `● @author · 2h ago` header,
  the body as markdown, then each reply as `↳ @author · time`.

A resolved thread is gone. An outdated one stays, marked `(outdated)`; one the
current diff has no line for reads at the top of the file, with the line it was
written on. The comments of your own unsubmitted review are not repeated there:
they are the pending ones, which you can still edit. A thread you are not
stopped on folds to 12 rows plus a `… N more lines` count.

`Enter` on a thread answers it the only way this tool writes: one more comment
of your pending review, on the same line. Nothing is posted until you finish the
review. The threads load with the PR and reload with `r`.

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
- A file the PR does not have — one only an unpushed commit or a pending edit
  touches — is marked here only: GitHub refuses a path that is not on the PR
  (`Filepath must be part of pull request`). Pushing re-marks the pushed files
  for real, so the mark carries over.
- A mark GitHub gave us is dropped for any file whose local change is not the
  one the PR has: it stood for the version the PR has, and a file already
  ticked off is one you will not look at again. The two diffs are compared
  line by line, not the commit lists — a rebase rewrites every commit without
  changing a thing, and only the added and removed lines are read, so context
  size and blob shas do not count. A mark made in this session stands, since
  it was made looking at the local state.
- Every `git` call reads the checkout the PR is open in, not the directory the
  app was started from. Those are two different branches whenever you review a
  PR from another repo checkout.
- A commit made from the Pending-edits pane re-reads the PR: it leaves that
  pane and joins the commit list and the diff in the same pass. The cursor
  stays on the file it was on.
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
| `syntax` | syntect parsing of diff code, cached per file |
| `term` | tty queries the terminal answers itself |
| `omarchy` | the active Omarchy theme palette |
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
