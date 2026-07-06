"""gh-review-ui — a minimal, lazygit-style GitHub PR review TUI.

The package is split into focused modules:

  gh          thin `gh` CLI / GraphQL subprocess wrappers
  api         GitHub domain calls built on `gh` (PRs, files, diffs, reviews)
  models      dataclasses + the central ``State`` object
  diff        unified-diff parsing and hunk indexing
  markdown    markdown/HTML → styled terminal lines
  tree        file-tree building and folding
  navigation  cursor/hunk/selection logic over ``State`` (pure)
  keys        keyboard decoding (modifier+Enter, flow control)
  theme       color pairs + generic highlight/coloring helpers
  textbuffer  the modal text editor + soft-wrapping
  render      curses drawing primitives and the panes
  modals      the comment / finish-review modal loops
  editor      external-editor integration
  worker      background thread running blocking `gh` jobs
  controller  state transitions + job orchestration
  app         curses bootstrap, the main loop, entry point

Almost all logic lives in the curses-free modules so it can be unit-tested.
"""
