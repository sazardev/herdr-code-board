# Agent guide

Notes for agents working on `herdr-code-board`. Read this before touching
anything that talks to Herdr.

## The rule that matters most: never read pane scrollback

The herdr-agent-quota plugin measured this and documented it, and it applies to
us just as hard:

| `pane read --source` | cost | repaints the user's pane |
| --- | --- | --- |
| `visible` | 0.006 s | no |
| `detection` | 0.004 s | no |
| `recent` | 4.452 s | **yes** |
| `recent-unwrapped` | 4.448 s | **yes** |

One `recent` read is one visible scroll in a pane a human is watching. This
plugin reacts to *every* agent status change on the machine, so a `recent` read
anywhere in the event path would repaint panes twice per turn, per agent.

`HerdrApi` therefore exposes only `read_visible`. Do not add `read_recent`. There
is exactly one call site (`Executor::answer_dialog`), it reads one pane, and only
when a rule is about to answer a dialog on the user's behalf.

The scroll is invisible to sampling: the repaint ends with the pane exactly as it
was, so content hashes match and `offset_from_bottom` never moves. Do not
conclude "no scroll happened" from either. A human has to watch the pane.

## Architecture, and why it is shaped this way

Three processes share one SQLite database:

| Process | Entry point | Lifetime | May dispatch? |
| --- | --- | --- | --- |
| event hook | `event` | one-shot, per herdr event | yes |
| timer daemon | `daemon` | long-lived, singleton | yes |
| TUI | `board` | while the pane is open | only through `sweep_once` |

Herdr fires `[[events]]` hooks globally, so the main path needs no daemon at all:
the hook advances the affected card and sweeps the ready queue. The daemon exists
only for rules that fire because nothing happened.

That means several processes can want to start an agent at the same instant.
`DispatchLock` (`src/engine/lock.rs`) is an advisory `flock` that makes the
dispatch section single-threaded across processes. **Every path that starts an
agent must hold it.** Skipping it lets two hooks claim the same pane.

`pane.agent_status_changed` fires twice per turn (idle→working on submit,
working→idle on completion). Anything the `event` path does, the user pays for
twice every time they press Enter. Budget accordingly.

## The reducer is pure; keep it that way

`src/engine/reducer.rs` is a function from `(card, rules, input)` to a list of
effects. No I/O, no database, no herdr. That is what makes the whole automation
story testable without a running server, and it is where every behavioural test
lives. New behaviour goes in the reducer with a test; the executor only carries
effects out.

`src/herdr/fake.rs` is the other half of that: an in-memory herdr with a call
log. `tests/engine_lifecycle.rs` runs the full lifecycle against it.

## Things that are easy to get wrong

- **`status_since` is a clock, not a timestamp.** `Store::set_lane` only advances
  it on a real lane change, because the `waiting_for` rules measure from it. A
  redundant write must not reset it.
- **Count the attempt before anything can fail.** `dispatch` calls
  `mark_dispatched` first. If placement fails after the attempt is counted, the
  retry budget still shrinks; if it failed before, the engine would loop forever.
- **Re-read the card after effects.** Effects run in sequence and earlier ones
  move the card. `Executor::apply` re-reads before running a rule action.
- **Consume the rule budget before running the action.** Otherwise an action that
  errors leaves the rule able to fire again immediately.
- **`prompt_sent` means "the handover finished"**, not "text was sent". A card
  with an empty prompt still sets it, or an idle agent would never move the card
  out of `running`.
- **Target agents by pane id.** Herdr's agent records do not reliably echo back
  the name assigned by `agent start`. `binding.pane_id` always resolves.
- **Auto-answer needs two switches.** `config.allow_auto_answer` *and*
  `card.auto_answer`. Do not collapse them, and do not add a third way in.

## Bugs the fake could not have caught

Three defects survived 140 passing tests and only showed up the first time this
ran against a live herdr with a real agent. They are fixed and pinned by tests
now, but they are the shape of thing to watch for:

1. **A card blocked at startup skipped its own rules.** `dispatch` set the lane
   to `blocked` directly on `agent_not_ready`, bypassing the reducer, so the
   `on blocked -> answer` rule — the entire reason that rule exists — never
   fired. Dispatch now routes that through `feed`.
2. **Such a card then never got its prompt.** `prompt_sent` was false and the
   agent went idle, which the reducer read as "still booting". It now emits
   `Effect::DeliverPrompt` for a live card whose handover never completed.
3. **Closing a run erased its notes.** `finish_open_run(.., None)` wrote a NULL
   over the dialog text recorded before an auto-answer. Both finishers now
   preserve, and append to, the existing detail.

The lesson: the fake models the happy path you thought of. Anything involving an
agent's *startup* — dialogs, trust prompts, slow boots — needs a real run.

## Herdr facts this depends on

Verified against herdr 0.8.2:

- Response envelope is `{"id": ..., "result": {...}}`; errors are JSON on stderr
  with exit status 1, syntax errors exit 2.
- Workspace records carry **no cwd**. A repo is matched to a workspace through
  the `cwd` / `foreground_cwd` of the panes inside it (`placement::find_workspace`).
- `agent start` requires a pane whose shell is in the foreground.
  `ProcessInfo::is_at_prompt` checks that; a freshly split pane always qualifies.
- `agent start` returning `agent_not_ready` is not a failure: the agent is up but
  on a startup dialog, and the name stays usable.
- A subscription to `pane.agent_status_changed` over the raw socket requires a
  specific `pane_id`. Manifest `[[events]]` hooks are global, which is why this
  plugin uses hooks rather than a socket subscription.
- Agent names must match `[a-z][a-z0-9_-]{0,31}` and be unique among live agents.
  `Card::slug` enforces both, with a test.
- `HERDR_PLUGIN_CONTEXT_JSON` for an action is **flat**, with prefixed keys
  (`focused_pane_cwd`, `workspace_cwd`) — not a nested pane object. Looking for a
  bare `cwd` finds nothing and silently falls back to the plugin's own directory.
- Claude Code's folder-trust dialog is an arrow list with no numbered shortcuts,
  which is why `Answer` navigates instead of typing a digit.

## Repository discovery

`src/git.rs` walks the filesystem itself rather than shelling out per directory:
`.git/HEAD` is one small read per repo, which is why a 58-repo home directory
scans in ~90 ms. Only `branches()` runs git, and only for one repo the user has
already chosen. Keep it that way — a scan happens on every press of `t`.

The `App` performs no I/O, so the picker is a two-step dance: `on_key` returns
`Request::ScanRepos(target)`, the run loop scans and calls `App::open_picker`
with the results. Same for branches (`Request::LoadBranches` → `set_branches`).
Do not reach for the filesystem from inside `state.rs`; the key handling tests
depend on it staying pure.

## Writing into herdr's own config

`src/integrate.rs` edits `~/.config/herdr/config.toml` **textually**, never
through a TOML round-trip. That file is shared: this machine already carries
herdr-agent-quota's five sidebar rows and the user's own comments, and
`toml::to_string` would reformat everything and drop every comment. So the code
finds the byte span of the `rows = …` value by bracket-balancing, splices, and
leaves the rest of the file alone. Its tests assert on a real config with
agent-quota's rows in it.

Everything written carries the `herdr-code-board` marker so uninstall can find
it. `write` refuses output that does not parse, and `apply` backs up first.
This is never automatic — it is an explicit action, because it is the user's file.

## Publishing to the sidebars

`src/engine/present.rs` is the only thing that calls `report_pane_tokens` /
`report_workspace_tokens`. Two rules:

1. **Only on change.** The last published set lives in `kv`; equal means no call.
   A metadata write can repaint a pane, and this plugin runs on every agent state
   change on the machine — twice per turn, per agent.
2. **Publish once per invocation**, at the end, not once per effect. `event` and
   `sweep_once` each call `Executor::present` exactly once.

A target that cannot be written to is dropped from the published set rather than
retried forever, and one failure never aborts the sweep — panes die, and herdr
sessions stop.

## Paths

There is one board per user. Herdr injects `HERDR_PLUGIN_CONFIG_DIR` and
`HERDR_PLUGIN_STATE_DIR` for hooks and panes; a plain shell gets neither, so
`Paths::resolve` reconstructs herdr's own layout — `herdr plugin config-dir <id>`
for config, `$XDG_STATE_HOME/herdr/plugins/<id>` for state. Do not "simplify"
that back into an XDG directory under our own name: it silently creates a second,
empty database, and the board looks wiped.

## Before you call it done

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

For anything touching dispatch, also run it for real in an **isolated** session,
never the user's main one:

```sh
herdr --session board-test
```
