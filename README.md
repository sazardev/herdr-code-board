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
herdr-code-board configure --apply   # via: herdr plugin action invoke herdr-code-board.configure
```

`configure --apply` is what makes it usable: it puts the CLI on your `PATH`,
adds the sidebar rows and binds the leader keys. Until you run it, the binary
only exists inside herdr's plugin directory.

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
herdr-code-board add "Review the diff" -p "Review the current diff and report only actionable findings." --start
herdr-code-board open
```

That is the whole setup. The card attaches to whatever repository you are
standing in, tracking it on the board the first time — you do not have to
register anything first.

## Repositories

The board finds your checkouts rather than asking you to type paths. It walks
your home directory (and `~/.config`, where editor and dotfile repos live),
skipping `node_modules`, build output and hidden directories, and lists what it
finds newest-first — 58 repos in about 90 ms here.

```sh
herdr-code-board repo scan            # what is out there, and what is tracked
herdr-code-board repo scan --add      # track all of it
herdr-code-board repo add             # track the repo you are standing in
herdr-code-board repo add shiki       # or one by name, wherever it lives
herdr-code-board repo ls              # tracked repos, branch, cards, concurrency
```

In the board, **`t`** opens the picker: every checkout on disk, with its current
branch, `●` for tracked and `○` for not. Type to filter — it is a subsequence
match, so `hcb` finds `herdr-code-board`. Enter tracks it and filters the board
to it. The same picker opens from the repo field of the new-card form, and a new
card starts in whichever repo you are already filtered to.

Point it elsewhere in `config.toml` if your code does not live under `$HOME`:

```toml
scan_roots = ["~/work", "/srv/checkouts"]
scan_depth = 4
```

## Branches

A `worktree` card cuts a branch, so it needs to know from where. Leave it alone
and it branches from wherever the repo is right now — which is what you mean
when you queue work from a checkout you are looking at:

```sh
herdr-code-board add "Risky refactor" --placement worktree --start
#   worktree board/risky-refactor-cm9735 from main
```

Name a base and it is checked against the repo's real branches before the card
is created, rather than failing at dispatch time:

```sh
$ herdr-code-board add "x" --placement worktree --base nosuch
Error: "nosuch" is not a branch of oxid. Available:
  main
  fix/scale-to-zero-and-deploy-visibility
```

In the form, `placement = worktree` reveals a **branch** field and a **from**
chooser listing that repo's actual branches, most recently committed first.

## Inside herdr

Run this once and the board stops being a separate app:

```sh
herdr-code-board configure --apply
```

**Leader keys.** `prefix+b` opens the board, `prefix+shift+b` opens a popup that
takes one line and queues it, `prefix+alt+b` re-imports overlays. They are
ordinary `[[keys.command]]` bindings, so they show up in herdr's own key help.

**The Agent sidebar.** Every pane a card owns gets the card's name, what it will
start when it finishes, and its attempt count — beside the agent, where you are
already looking:

```text
● Claude/Opus · rustock
  ▶ Review the diff  → Run the tests
```

**The Spaces sidebar.** Each workspace shows how much of the board is live in it:

```text
▣ rustock   main
  ▶ 2 · ◷ 3
```

**Notifications.** A card reaching `blocked` or `failed` raises a herdr
notification with the right sound. Change which lanes interrupt you:

```toml
notify_on = ["blocked", "failed", "done"]
notifications = true
sidebar = true
```

`configure --apply` edits `~/.config/herdr/config.toml` textually — it appends
one row to each sidebar and three keybindings, backs the file up first, and
refuses to write anything herdr could not parse. Your comments and other
plugins' rows are left byte for byte. `configure` on its own reports what is
wired; `configure --uninstall` takes back exactly what it added.

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
prefix+b        open the board (after `configure --apply`)
prefix+shift+b  popup: type one line, it runs

a         quick add — one line, queued immediately
n         new card, full form
space     queue / unqueue        Q    queue the whole lane
enter     jump to the herdr pane
v         detail: rules, runs, log
c         chain this card to another one
e         edit the card          E    edit the prompt in $EDITOR
y         duplicate

h l ← →   lanes                  1..9 jump to a lane
j k ↑ ↓   cards                  g G  first / last
H L       shift a lane over      J K  move up/down the lane

t         pick a repository      tab  cycle the repo filter
/         search

x         cancel and release the pane
r         re-dispatch from scratch
d         delete (asks first)
s         re-import overlays     R    reload    ?  help    q  quit
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

`--branch` names the branch (`{card}` expands to the card slug) and `--base`
what to cut it from; see [Branches](#branches).

Split direction follows herdr's own geometry rule: wide panes split right, tall
panes split down. Override with `--direction right|down` and `--ratio`.

### The fast path

`a`, type a line, enter. The card lands in whichever repo the board is filtered
to, with that repo's default agent, and starts immediately. That is the whole
loop for "run this against that repo, now".

For anything with structure, `n` opens the form. `E` on any card hands its
prompt to `$EDITOR` and takes the terminal back when you are done — a prompt is
the one field that genuinely wants more than one line.

## Rules

Rules are what make the board a workflow rather than a list.

In the board, `c` chains the selected card to another: pick the follower, pick
the condition (`when it is done`, `when it fails`, `after waiting 15m`, …).
`v` shows everything a card carries — its prompt, its rules, its run history and
its log — and `d` there removes a rule.

From the shell:

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
- **A card can only be dispatched `max_dispatches` times (25 by default).** Rules
  can form a cycle — A queues B, B queues A — and every pass is a real agent
  burning real quota. When a card hits the ceiling it stops in `failed` and says
  so; `retry` clears the count.
- **Metadata writes can repaint a pane.** The board publishes tokens only when a
  value actually changed, and never reads pane scrollback. If you see panes
  scrolling, `sidebar = false` in `config.toml` turns publishing off.
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
