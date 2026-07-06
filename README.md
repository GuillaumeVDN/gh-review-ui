# gh-review-ui

Minimal, lazygit-style terminal UI for reviewing GitHub PRs.

Four panes (left column stacked, right side full-height):
- **PRs** — open PRs you authored *or* have been requested to review.
- **Pending** — review comments queued locally, waiting to be submitted.
- **Files** — file tree of the currently checked-out PR, with viewed-state.
- **Right** — PR description + timeline when the PRs pane is focused,
  diff of the highlighted / opened file otherwise. The current hunk is
  marked with a green side-bar.

Backed by the `gh` CLI (for auth, PR list, checkout, diff) plus GitHub's GraphQL
API for the `viewedState` mutations. Zero Python dependencies — pure stdlib
`curses` + threads.

## Requirements

- Python 3.10+
- [`gh` CLI](https://cli.github.com/) — authenticated (`gh auth login`)
- A terminal that supports 256 colors and mouse events (foot, kitty, alacritty, wezterm, ghostty, xterm…)

## Install

Clone anywhere and either symlink or alias the executable:

```sh
git clone https://github.com/GuillaumeVDN/gh-review-ui.git ~/Projects/github-pr-view-ui

# option A — symlink to a bin directory already on your PATH
ln -s ~/Projects/github-pr-view-ui/gh-review-ui ~/.local/bin/gh-review-ui

# option B — shell alias
echo 'alias ghr="~/Projects/github-pr-view-ui/gh-review-ui"' >> ~/.bashrc
```

## Run

From any directory inside a GitHub repo checkout:

```sh
gh-review-ui
```

On start it will:
1. detect the repo from the current working directory (via `gh repo view`);
2. fetch open PRs where you're the author or a requested reviewer;
3. if the current branch corresponds to a PR, load its file list and diff.

## Keys

Global:
- `Tab` / `Shift-Tab` — cycle panes (PRs → Pending → Files → Diff)
- `q` — quit
- `r` — refresh PR list + active PR (also reloads details when on the PRs pane)
- `Shift+J` / `Shift+K` — scroll the diff one line (works from any pane)
- `Shift+C` — comment on the current hunk (opens a modal editor)
- `Shift+F` — finish review (submit all pending comments)
- mouse wheel — scroll the pane under the cursor (independent of selection)
- click — focus a pane

PRs pane:
- `j` / `k` / arrows — move
- `Enter` — `gh pr checkout` the selected PR
- `d` / `u` or PgDn / PgUp — scroll the details view

Pending pane:
- `j` / `k` — move
- `d` — discard the highlighted pending comment

Files pane:
- `j` / `k` — move
- `Space` — collapse / expand folder
- `v` — toggle viewed on file, or on all files under a folder
- `Enter` — open file in the diff pane (focuses diff)

Diff pane:
- `j` / `k` / arrows — jump to next / previous hunk
- `d` / `u` or PgDn / PgUp — page down / up
- `g` / `G` — top / bottom
- `Shift+C` — comment on the current hunk
- `Esc` — back to the files pane

### Comment modal (`Shift+C` in diff pane)

Opens a multi-line text editor anchored to the first commentable line in the
current hunk (`RIGHT` side by default, `LEFT` if the hunk is a pure deletion).

- typing — insert text
- `Enter` — insert a newline
- `Backspace` — delete character
- Arrow keys / `Home` / `End` — move cursor
- `Ctrl+S` — add to the pending review
- `Ctrl+X` — send *now* as a single-comment review (event = `COMMENT`)
- `Esc` — cancel

### Finish-review modal (`Shift+F`)

Two-step:

1. Multi-line body editor. `Ctrl+S` continues, `Esc` cancels.
2. Choice screen: `a` approve · `c` comment · `r` request changes · `Esc` cancel.

On submit, a review is created via GraphQL with all pending comments as
inline threads, then submitted with the chosen event.

## Notes

- "Viewed" state is stored server-side on GitHub; toggling here syncs to the PR review UI on github.com.
- Diffs are fetched once per checkout — press `r` to reload after new pushes.
- File pagination handles PRs with up to a few hundred files.
