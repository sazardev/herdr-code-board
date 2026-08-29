# Changelog

## 0.4.0

The board lives inside herdr now, not beside it.

- `configure --apply` wires it in: `prefix+b` opens the board, `prefix+shift+b`
  opens a popup that takes one line and queues it, `prefix+alt+b` re-imports
  overlays. Ordinary `[[keys.command]]` bindings, so they appear in herdr's key
  help.
- Every pane a card owns publishes `$board_card`, `$board_next` and
  `$board_meta` into the Agent sidebar; every workspace publishes `$board_space`
  into the Spaces sidebar. Tokens are written only when a value actually
  changed, because a metadata write can repaint a pane a human is watching.
- Cards reaching `blocked` or `failed` raise a herdr notification with the
  matching sound. Configurable through `notify_on`.
- The config edit is textual and surgical: it appends one row per sidebar and
  three keybindings, preserves other plugins' rows and every comment, backs the
  file up, and refuses to write a config herdr could not parse.
  `configure --uninstall` takes back exactly what it added.
- A pane or workspace that disappears is forgotten instead of failing every
  later publish.

## 0.3.0

The board is the interface; the CLI is the fallback.

- `a` is one-line capture: type a prompt, press enter, it queues and runs in
  whichever repo the board is filtered to.
- `c` chains cards from inside the board — pick the follower, pick the
  condition. No more dropping to a shell to write a rule.
- `v` opens a card in full: prompt, rules (named, not "1 card(s)"), run history
  with the recorded dialog text, and its event log. `d` removes a rule there.
- `E` edits a prompt in `$EDITOR`, suspending and restoring the terminal.
- `y` duplicates a card, `J`/`K` reorder within a lane, `Q` queues a whole lane,
  and `1`..`9` jump straight to one.
- The TUI holds one SQLite connection for the life of the pane instead of
  opening a fresh one on every keystroke and every poll tick.

## 0.2.0

Repositories are found, not typed.

- `repo scan` walks your home directory for checkouts and reports each one with
  its current branch and whether the board tracks it. ~90 ms for 58 repos;
  `.git/HEAD` is read directly rather than invoking git per directory.
- `t` in the board opens a searchable picker over everything found, with
  subsequence matching (`hcb` → `herdr-code-board`). It also backs the repo
  field of the new-card form.
- `add` now attaches the card to the repository you are standing in, tracking it
  on the first use. Previously it silently created a repo-less card.
- `repo add` accepts a project name as well as a path, and outside a checkout it
  lists what it found instead of failing.
- A `worktree` card with no `--base` branches from wherever the repo is now,
  resolved in the engine so every entry point agrees. A named base is validated
  against the repo's real branches at creation time, with the list in the error.
- The form grows `branch` and `from` fields when the placement is `worktree`,
  the latter a chooser over that repo's branches.
- `repo ls` shows branch, card counts and live counts.

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
