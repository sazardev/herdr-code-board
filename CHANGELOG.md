# Changelog

## 0.1.0

First release.

- Kanban TUI with nine lanes, a new/edit card form, search, repo filtering and
  live updates driven by a database revision counter rather than pane reads.
- Dispatch engine that resolves a card's placement into real herdr topology —
  split pane, new tab, new workspace, or a git worktree — starts the agent and
  delivers the prompt.
- Rule engine: `done`, `review`, `failed`, `blocked` triggers plus the timed
  `waiting for` and `blocked for` forms; actions to enqueue other cards, prompt,
  answer a dialog, send keys, notify, retry, cancel, or close the pane.
- Per-repo concurrency limits, retry budgets, and a run history per card.
- `.herdr-board.toml` repo overlays, imported idempotently by `(repo, key)`.
- Auto-answering approval dialogs behind two independent opt-ins, with the
  dialog text recorded before the answer is sent.
- `doctor` reports which agent model flags were verified and which are assumed.

Verified end to end against herdr 0.8.2 in an isolated session: a card started a
real agent, its `on blocked` rule answered Claude Code's folder-trust dialog, the
prompt was delivered once the dialog cleared, and the linked follow-up card was
queued and dispatched on its own. That run found three defects the in-memory fake
could not — see AGENTS.md.
