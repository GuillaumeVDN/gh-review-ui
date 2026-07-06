# gh-review-ui (`prtui`)

Minimal, lazygit-style terminal UI for reviewing GitHub PRs.

Three panes:
- **Top-left** — open PRs you authored *or* have been requested to review.
- **Bottom-left** — file tree of the currently checked-out PR, with viewed-state.
- **Right** — diff of the highlighted / opened file, hunk-navigable.

Backed by the `gh` CLI (for auth, PR list, checkout, diff) plus GitHub's GraphQL
API for the `viewedState` mutations. Zero Python dependencies — pure stdlib
`curses` + threads.

## Install

```sh
git clone https://github.com/GuillaumeVDN/gh-review-ui.git ~/Projects/github-pr-view-ui
chmod +x ~/Projects/github-pr-view-ui/prtui
alias prtui="~/Projects/github-pr-view-ui/prtui"
```

Requires: `gh` (authenticated via `gh auth login`) and Python 3.

## Use

Run `prtui` from any directory inside a GitHub repo checkout.

### Keys

Global:
- `Tab` / `Shift-Tab` — cycle panes
- `q` — quit
- `r` — refresh PR list + active PR
- `Shift+J` / `Shift+K` — scroll diff pane one line (any focus)
- mouse wheel — scroll the pane under the cursor (independent of selection)
- click — focus a pane

PRs pane:
- `j` / `k` / arrows — move
- `Enter` — `gh pr checkout` the selected PR

Files pane:
- `j` / `k` — move
- `Space` — collapse / expand folder
- `v` — toggle viewed on file, or on all files under a folder
- `Enter` — open file in the diff pane (focuses diff)

Diff pane:
- `j` / `k` / arrows — jump to next / previous hunk
- `d` / `u` or PgDn / PgUp — page down / up
- `g` / `G` — top / bottom
- `Esc` — back to files pane
