# herdr-code-board

A kanban queue for agentic prompts, inside [Herdr](https://herdr.dev).

Queuing agent work by hand is a loop: open a pane, start an agent, paste a prompt,
wait, notice it finished, decide what is next. This plugin turns that loop into a
board. You write the prompts once as cards — each with its repo, agent, model and
where it should run — and the engine does the rest: it creates the workspace,
splits the pane or cuts the worktree, starts the agent, delivers the prompt, and
when the card finishes it starts whichever card you linked to it.

```text
 Backlog      Ready       Running      Waiting     Blocked      Done
 ─────────    ─────────   ──────────   ─────────   ──────────   ─────────
 write docs   run tests   review diff  fix lint    migrate db   scaffold
 erp·claude   erp·codex   erp·claude   web·claude  erp·claude   erp·claude
                          w18:p2·3m    w19:p1·22m  w18:p5·1m
```

- **Rust TUI.** Nine lanes, vim keys, a form for new cards, live updates.
- **Real herdr topology.** Cards land in a split pane, a new tab, a fresh
  workspace, or their own git worktree — one column per repo if you want it.
- **Rules that chain work.** `when this is done, start that`. `if it has been
  waiting 15 minutes, poke it`. `if it hits an approval dialog, answer option 1`.
- **Repo-level or global.** Track repos, tag them, filter by them. Flows a team
  shares live in the repo as `.herdr-board.toml`, in git, in review.

Status: 0.1.0. It works end to end; the interfaces may still move.

## Install

Requires Herdr 0.8.0+, Rust 1.89+, Linux or macOS, and at least one agent CLI
that herdr recognizes.

```sh
herdr plugin install sazardev/herdr-code-board
```

Or, to hack on it locally:

```sh
git clone https://github.com/sazardev/herdr-code-board
cd herdr-code-board
./install.sh
```

`install.sh` builds the release binary and links the working tree, because
`herdr plugin link` deliberately does not run build commands.

To use the CLI from your own shell, put it on `PATH`:

```sh
ln -sf "$PWD/target/release/herdr-code-board" ~/.local/bin/
```

It finds herdr's own plugin directories on its own, so a plain shell and the
event hooks read and write the same board.

Then, from inside a repo:

```sh
herdr-code-board repo add
herdr-code-board add "Review the diff" --prompt "Review the current diff and report only actionable findings." --start
herdr-code-board open
```

## The board

| Lane | What it means |
| --- | --- |
| `backlog` | captured, not queued |
| `ready` | the engine will start it as soon as the repo has a free slot |
| `running` | the agent is working |
| `waiting` | the agent's turn ended and nobody has acted on it |
| `blocked` | herdr recognized an approval or question dialog |
| `review` | parked for a human, by you or by a rule |
| `done` `failed` `cancelled` | terminal |

By default a card completes when its agent's turn ends. Pass `--review` (or
`review = true` in an overlay) to park it in `waiting` for a human instead.

### Keys

```
h l ← →   lanes                 n   new card
j k ↑ ↓   cards                 e   edit
g G       first / last          x   cancel and release the pane
H L       shift a lane over     r   re-dispatch from scratch
space     queue / unqueue       d   delete (asks first)
enter     jump to the pane      /   search      tab  repo filter
s         re-import overlays    R   reload      ?    help      q  quit
```

## Placement

Where a card's agent runs:

| `--placement` | Effect |
| --- | --- |
| `split` (default) | split a new pane in the repo's workspace — the vertical column per card |
| `reuse` | take an idle pane if there is one, split only if there is not |
| `tab` | a new tab in the repo's workspace |
| `workspace` | a fresh workspace rooted at the repo |
| `worktree` | `herdr worktree create`, which herdr opens as its own workspace |

```sh
herdr-code-board add "Try the risky refactor" \
  --placement worktree --branch board/refactor --base main --start
```

Split direction follows herdr's own geometry rule: wide panes split right, tall
panes split down. Override with `--direction right|down` and `--ratio`.

## Rules

Rules are what make the board a workflow rather than a list.

```sh
# when "write the code" completes, queue "run the tests"
herdr-code-board link "write the code" "run the tests"

# if a card sits in waiting for 15 minutes, ask it what is going on
herdr-code-board link "slow one" --on waiting --after 15m
```

Triggers: `done`, `review`, `failed`, `blocked`, and the timed forms
`--on waiting --after 15m` and `--on blocked --after 5m`.

Actions available in an overlay file: `enqueue`, `prompt`, `answer`, `keys`,
`notify`, `retry`, `cancel`, `close_pane`.

### Answering blocked dialogs

`answer = 1` types into an approval dialog on your behalf. That is genuinely
risky — it approves something no human read — so it needs **two** independent
opt-ins:

```toml
# ~/.config/herdr/plugins/config/herdr-code-board/config.toml
allow_auto_answer = true
```

```sh
herdr-code-board add "trusted chore" --auto-answer ...
```

Both default to off. Before answering, the engine records what the dialog said
into the card's run history, so there is an audit trail of what got approved.

Answering **navigates** — `down` × (choice − 1), then `enter` — rather than
typing a digit. Agent TUIs disagree about numbered shortcuts (Claude Code's trust
prompt is a plain arrow list with no numbers at all), but they all agree on move
and confirm. That assumes the cursor starts on the first option, which is what
every dialog seen so far does. For anything else, use a `keys` rule and spell out
the key sequence yourself.

A card that comes up blocked on a startup dialog keeps its prompt in hand: the
prompt is delivered once the dialog clears and the agent is ready, not before.

## Repo overlays

A repo can carry its own cards in `.herdr-board.toml`. `sync` imports them
idempotently, keyed by `(repo, key)`, and never touches a card's live state — so
re-syncing while a card is running is safe.

```toml
[repo]
name = "erp"
tags = ["work"]
max_parallel = 2
agent = "claude"

[[card]]
key = "review-diff"
title = "Review the diff"
prompt = "Review the current diff and report only actionable findings."
start = true

  [[card.rules]]
  on = "done"
  enqueue = ["run-tests"]

  [[card.rules]]
  on = "waiting"
  after = "15m"
  notify = "review is stalled"
  max_fires = 1

[[card]]
key = "run-tests"
title = "Run the tests"
prompt = "Run the test suite and fix what fails."
placement = "worktree"
branch = "board/{card}"
base = "main"
review = true
retries = 2
```

See [`examples/.herdr-board.toml`](examples/.herdr-board.toml) for every field.

## How it works

Herdr's plugin API is just the herdr CLI, so this plugin is three small programs
sharing one SQLite database:

- **Event hooks.** Herdr runs `herdr-code-board event` on every
  `pane.agent_status_changed`, `pane.exited`, `pane.closed` and
  `workspace.closed`. The hook advances the affected card and then starts
  whatever now fits. This is the main path, and it needs no daemon.
- **The timer daemon.** Started by the plugin's startup hook. It exists only for
  rules that fire because *nothing* happened, and to re-sweep the ready queue.
- **The TUI.** Reads and edits; it never dispatches directly.

All three can run at once, so dispatch is wrapped in an advisory file lock: two
processes can never claim the same pane or start the same card twice.

**The board never reads pane scrollback.** `pane read --source recent` costs
~4.4 s and visibly repaints the pane the user is watching — measured and
documented by the herdr-agent-quota plugin. The only read this plugin performs is
`--source visible`, on one pane, and only to record what an approval dialog said
before auto-answering it.

## Model flags

Agent CLIs disagree about how to pass a model. The mapping lives in
`config.toml` and you can override any of it:

```toml
[model_flags]
codex = "-m"
opencode = ["--model", "{model}"]
```

Verified against the CLI's own `--help` on the machine this was built on:
`claude`, `opencode`, `copilot`. Assumed for `codex`, `gemini`, `cursor`, `qwen`,
`grok`, `droid`. Anything else falls back to `--model` and says so in the card's
event log. `herdr-code-board doctor` prints which is which.

## Commands

```
board            open the TUI (also the plugin's pane entrypoint)
add              add a card
repo add|ls|rm   track repositories
ls | show        list cards, inspect one
move | retry | cancel | rm
link             chain one card to another
sync             re-import .herdr-board.toml
doctor           check herdr, the database, the daemon and the model flags
daemon           run the timer daemon in the foreground
```

## Known limitations

- **The board is global; herdr sessions are not.** Cards live in one database
  for your user, but an event hook dispatches into whichever herdr session fired
  the event. With a single session — the normal case — that is invisible. If you
  run several named sessions at once, which one picks up a queued card is not
  determined.
- **The CLI is not installed on `PATH` by the plugin.** Herdr only needs the
  binary inside the plugin directory; symlink it yourself if you want the
  commands.
- **Timed rules need the daemon.** It is started by the plugin's startup hook. If
  you linked the plugin without restarting herdr, run `herdr-code-board startup`
  once. `doctor` tells you whether it is running.
- **Model flags for most agent kinds are assumed, not verified.** See above.

## Not in 0.1.0

- Windows (declared in the manifest's `platforms`)
- herdr sidebar integration via `agent.view.set`
- `[[link_handlers]]`, to enqueue a card from a clicked issue URL

## License

MIT
