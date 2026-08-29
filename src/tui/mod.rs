//! The kanban TUI.

pub mod form;
pub mod render;
pub mod state;
pub mod theme;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::agents;
use crate::config::{Config, Paths};
use crate::engine::daemon;
use crate::herdr::client::CliHerdr;
use crate::herdr::HerdrApi;
use crate::model::Column;
use crate::overlay;
use crate::store::Store;

use crate::store::cards::NewCard;
use state::{App, Detail, Key, PickerTarget, RepoChoice, Request};
use theme::Theme;

/// Translate a terminal event into the app's own key type.
fn decode(ev: Event) -> Option<Key> {
    let Event::Key(key) = ev else { return None };
    if key.kind != KeyEventKind::Press {
        return None;
    }
    Some(match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl-C should quit even from inside a modal.
            if c == 'c' {
                Key::Esc
            } else {
                Key::Char(c)
            }
        }
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        _ => return None,
    })
}

pub fn run(paths: &Paths, config: &Config, quick: bool) -> Result<()> {
    let herdr: Arc<dyn HerdrApi> = Arc::new(CliHerdr::new());
    let theme = Theme::from_config(&config.theme);

    let mut app = App::new(
        agents::KINDS.iter().map(|s| s.to_string()).collect(),
        config.default_agent.clone(),
    );
    if quick {
        app.start_quick();
    }

    let mut terminal = ratatui::init();
    let result = event_loop(paths, config, herdr, &mut terminal, &mut app, &theme);
    ratatui::restore();
    result
}

fn reload(store: &Store, app: &mut App) -> Result<()> {
    app.load(store.list_cards()?, store.list_repos()?);
    Ok(())
}

fn event_loop(
    paths: &Paths,
    config: &Config,
    herdr: Arc<dyn HerdrApi>,
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    theme: &Theme,
) -> Result<()> {
    let poll = Duration::from_millis(config.tui_poll_ms.max(50));
    // One connection for the life of the pane. Reopening it per keystroke, as
    // this used to, made every key press pay for a fresh SQLite handle.
    let mut store = Store::open(&paths.database())?;
    reload(&store, app)?;
    let mut revision = store.revision()?;
    let mut last_poll = Instant::now();

    loop {
        terminal.draw(|frame| render::draw(frame, app, theme))?;

        if event::poll(poll)? {
            if let Some(key) = decode(event::read()?) {
                let request = app.on_key(key);
                if request == Request::Quit {
                    return Ok(());
                }
                let mutating = request != Request::None;

                // Editing leaves the terminal to `$EDITOR` and comes back.
                if let Request::EditPrompt(card_id) = &request {
                    let card_id = card_id.clone();
                    match edit_prompt(&mut store, terminal, &card_id) {
                        Ok(true) => app.status = "prompt saved".into(),
                        Ok(false) => app.status = "prompt unchanged".into(),
                        Err(e) => app.status = format!("{e:#}"),
                    }
                } else if let Err(e) =
                    execute(&mut store, paths, config, herdr.clone(), app, request)
                {
                    app.status = format!("{e:#}");
                }

                // Navigation changes nothing on disk, and re-reading every card
                // for a `j` is pure waste at any real board size.
                if mutating {
                    reload(&store, app)?;
                    revision = store.revision()?;
                }
                // The quick popup exists to capture one prompt and get out of
                // the way.
                if app.oneshot && app.mode == state::Mode::Normal {
                    return Ok(());
                }
                continue;
            }
        }

        // Cheap change detection: the engine and the hooks bump the board's
        // revision, so the TUI never has to read a pane to stay current.
        if last_poll.elapsed() >= poll {
            last_poll = Instant::now();
            let current = store.revision()?;
            if current != revision {
                revision = current;
                reload(&store, app)?;
            }
        }
    }
}

/// Hand the card's prompt to `$EDITOR`, then take the terminal back.
///
/// A prompt is the one field that genuinely wants more than a single line, and
/// reimplementing a text editor inside a kanban board would be the wrong answer.
fn edit_prompt(
    store: &mut Store,
    terminal: &mut ratatui::DefaultTerminal,
    card_id: &str,
) -> Result<bool> {
    let Some(mut card) = store.get_card(card_id)? else {
        return Ok(false);
    };
    let file = std::env::temp_dir().join(format!("herdr-board-{card_id}.md"));
    std::fs::write(&file, &card.prompt)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    // Give the terminal back before handing it to someone else, and always take
    // it again — even if the editor fails — or the pane is left unusable.
    ratatui::restore();
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\"",))
        .arg("sh")
        .arg(&file)
        .status();
    *terminal = ratatui::init();
    terminal.clear()?;
    status?;

    let edited = std::fs::read_to_string(&file)?;
    let _ = std::fs::remove_file(&file);
    if edited == card.prompt {
        return Ok(false);
    }
    card.prompt = edited;
    store.update_card(&card)?;
    Ok(true)
}

fn execute(
    store: &mut Store,
    paths: &Paths,
    config: &Config,
    herdr: Arc<dyn HerdrApi>,
    app: &mut App,
    request: Request,
) -> Result<()> {
    match request {
        Request::None | Request::Quit => {}
        // Handled by the caller, which owns the terminal.
        Request::EditPrompt(_) => {}
        Request::Reload => {}

        Request::SetLane { card_id, lane } => {
            store.set_lane(&card_id, lane)?;
            if lane == Column::Ready {
                sweep(paths, config, herdr, app);
            }
        }

        Request::Cancel { card_id } => {
            if let Some(card) = store.get_card(&card_id)? {
                store.finish_open_run(&card.id, "cancelled", Some("from the board"))?;
                store.clear_binding(&card.id)?;
                store.set_lane(&card.id, Column::Cancelled)?;
                app.status = format!("{} cancelled", card.title);
            }
        }

        Request::Retry { card_id } => {
            store.clear_binding(&card_id)?;
            store.reset_rule_fires(&card_id)?;
            store.reset_attempts(&card_id)?;
            store.set_error(&card_id, None)?;
            store.set_lane(&card_id, Column::Ready)?;
            sweep(paths, config, herdr, app);
        }

        Request::Delete { card_id } => {
            if let Some(card) = store.get_card(&card_id)? {
                if card.column.is_live() {
                    app.status = "that card is still running; cancel it first".into();
                } else {
                    store.delete_card(&card_id)?;
                }
            }
        }

        Request::FocusPane { pane_id } => {
            // `agent focus` would require the pane to host a recognized agent;
            // focusing the pane works whatever is in it.
            CliHerdr::new()
                .call_raw(&["pane", "zoom", &pane_id, "--off"])
                .ok();
            CliHerdr::new().call_raw(&["pane", "get", &pane_id])?;
            CliHerdr::new().call_raw(&["agent", "focus", &pane_id]).ok();
            app.status = format!("focused {pane_id}");
        }

        Request::Sync => {
            let mut created = 0;
            let mut updated = 0;
            for repo in store.list_repos()? {
                let path = std::path::PathBuf::from(&repo.path);
                if !path.exists() {
                    continue;
                }
                match overlay::sync_repo(store, &path, &config.default_agent) {
                    Ok(r) => {
                        created += r.created;
                        updated += r.updated;
                    }
                    Err(e) => app.status = format!("{}: {e:#}", repo.name),
                }
            }
            if app.status.is_empty() {
                app.status = format!("sync: {created} created, {updated} updated");
            }
        }

        Request::Create(card) => {
            let created = store.create_card(&card)?;
            if created.column == Column::Ready {
                sweep(paths, config, herdr, app);
            }
            app.status = format!("added {}", created.title);
        }

        Request::Update(card) => {
            store.update_card(&card)?;
            app.status = format!("saved {}", card.title);
        }

        Request::ScanRepos(target) => {
            let items: Vec<RepoChoice> = crate::app::scan(store, config.roots())?
                .into_iter()
                .map(|c| RepoChoice {
                    name: c.found.name,
                    path: c.found.path,
                    branch: c.found.branch,
                    tracked: c.tracked,
                })
                .collect();
            app.open_picker(items, target);
        }

        Request::UseRepo { path, target } => {
            // Tracking is idempotent, so picking an already-tracked repo just
            // refreshes its overlay.
            overlay::sync_repo(store, &path, &config.default_agent)?;
            app.load(store.list_cards()?, store.list_repos()?);
            match target {
                PickerTarget::Filter => {
                    app.filter_by_path(&path);
                    app.status = format!("showing {}", app.filter_label());
                }
                PickerTarget::Form => {
                    app.form_repo_by_path(&path);
                    app.set_branches(crate::git::branches(&path).unwrap_or_default());
                    let branch = crate::git::head_branch(&path);
                    app.status = format!(
                        "card will run in {} on {}",
                        path.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        branch.as_deref().unwrap_or("(detached)")
                    );
                }
            }
        }

        Request::LoadBranches(path) => {
            app.set_branches(crate::git::branches(&path).unwrap_or_default());
        }

        Request::QuickAdd(text) => {
            // Whatever repo the board is filtered to is the one you mean.
            let repo = app
                .repo_filter
                .and_then(|i| app.repos.get(i))
                .map(|r| r.id.clone());
            let agent = app
                .repo_filter
                .and_then(|i| app.repos.get(i))
                .and_then(|r| r.default_agent.clone())
                .unwrap_or_else(|| config.default_agent.clone());
            let card = store.create_card(&NewCard {
                title: crate::app::ellipsize(&text, 60),
                prompt: text,
                repo_id: repo,
                column: Column::Ready,
                ..NewCard::new("", agent)
            })?;
            app.status = format!("queued {}", card.title);
            sweep(paths, config, herdr, app);
        }

        Request::Duplicate(card_id) => {
            if let Some(card) = store.get_card(&card_id)? {
                let copy = store.create_card(&NewCard {
                    // A copy is never an overlay card: that key belongs to the
                    // original, and two cards cannot share it.
                    key: None,
                    title: format!("{} copy", card.title),
                    prompt: card.prompt.clone(),
                    repo_id: card.repo_id.clone(),
                    tags: card.tags.clone(),
                    agent_kind: card.agent_kind.clone(),
                    model: card.model.clone(),
                    extra_args: card.extra_args.clone(),
                    placement: card.placement.clone(),
                    column: Column::Backlog,
                    priority: card.priority,
                    auto_complete: card.auto_complete,
                    auto_answer: card.auto_answer,
                    max_retries: card.max_retries,
                })?;
                app.status = format!("copied to {}", copy.title);
            }
        }

        Request::Reorder { card_id, delta } => {
            store.reorder_in_lane(&card_id, delta)?;
        }

        Request::QueueLane(ids) => {
            let mut queued = 0;
            for id in ids {
                if store.set_lane(&id, Column::Ready)? {
                    queued += 1;
                }
            }
            app.status = format!("queued {queued}");
            sweep(paths, config, herdr, app);
        }

        Request::LoadDetail(card_id) => {
            if let Some(card) = store.get_card(&card_id)? {
                let rules = store
                    .rules_for_card(&card.id, card.repo_id.as_deref())?
                    .into_iter()
                    .map(|r| {
                        let budget = if r.max_fires > 0 {
                            format!("  ({}/{})", r.fired, r.max_fires)
                        } else {
                            String::new()
                        };
                        // Name the cards a rule queues. "queue 2 card(s)" tells
                        // you nothing when you are looking at a chain.
                        let action = match &r.action {
                            crate::model::Action::Enqueue { cards } => {
                                let titles: Vec<String> = cards
                                    .iter()
                                    .map(|id| {
                                        store
                                            .get_card(id)
                                            .ok()
                                            .flatten()
                                            .map(|c| c.title)
                                            .unwrap_or_else(|| format!("{id} (missing)"))
                                    })
                                    .collect();
                                format!("run {}", titles.join(", "))
                            }
                            other => other.describe(),
                        };
                        (r.id, format!("{} → {action}{budget}", r.trigger.describe()))
                    })
                    .collect();
                let runs = store
                    .runs_for_card(&card.id, 8)?
                    .into_iter()
                    .map(|r| {
                        format!(
                            "#{} {} · {} ago{}",
                            r.attempt,
                            r.outcome.as_deref().unwrap_or("open"),
                            crate::app::ago(r.started_at),
                            r.detail
                                .as_deref()
                                .map(|d| format!("\n    {}", d.replace('\n', "\n    ")))
                                .unwrap_or_default()
                        )
                    })
                    .collect();
                let events = store
                    .recent_events(40)?
                    .into_iter()
                    .filter(|e| e.card_id.as_deref() == Some(card.id.as_str()))
                    .take(10)
                    .map(|e| {
                        format!(
                            "{} ago  {}{}",
                            crate::app::ago(e.at),
                            e.kind,
                            e.detail
                                .as_deref()
                                .map(|d| format!(": {d}"))
                                .unwrap_or_default()
                        )
                    })
                    .collect();
                app.open_detail(Detail {
                    card_id: card.id.clone(),
                    title: card.title.clone(),
                    prompt: card.prompt.clone(),
                    rules,
                    runs,
                    events,
                    cursor: 0,
                });
            }
        }

        Request::Chain { from, to, trigger } => {
            store.add_rule(
                Some(&from),
                None,
                &trigger,
                &crate::model::Action::Enqueue {
                    cards: vec![to.clone()],
                },
                0,
            )?;
            let title = store
                .get_card(&to)?
                .map(|c| c.title)
                .unwrap_or_else(|| to.clone());
            app.status = format!("{} → {title}", trigger.describe());
        }

        Request::DeleteRule(rule_id) => {
            store.delete_rule(&rule_id)?;
            // Keep the overlay open and current rather than dropping the user
            // back to the board after every removal.
            if let Some(detail) = app.detail.clone() {
                app.detail = None;
                return execute(
                    store,
                    paths,
                    config,
                    herdr,
                    app,
                    Request::LoadDetail(detail.card_id),
                );
            }
        }
    }
    Ok(())
}

/// Start whatever is ready, without letting a herdr failure kill the TUI.
fn sweep(paths: &Paths, config: &Config, herdr: Arc<dyn HerdrApi>, app: &mut App) {
    match daemon::sweep_once(paths, config, herdr) {
        Ok(true) => {}
        Ok(false) => app.status = "another dispatch is in flight; queued".into(),
        Err(e) => app.status = format!("dispatch failed: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEvent, KeyEventState};

    fn press(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn key_releases_are_ignored_so_a_press_does_not_act_twice() {
        let release = Event::Key(KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        assert_eq!(decode(release), None);
        assert_eq!(
            decode(press(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(Key::Char('j'))
        );
    }

    #[test]
    fn ctrl_c_is_treated_as_escape_so_it_always_gets_you_out() {
        assert_eq!(
            decode(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Key::Esc)
        );
    }

    #[test]
    fn unmapped_keys_are_dropped_rather_than_guessed() {
        assert_eq!(decode(press(KeyCode::F(5), KeyModifiers::NONE)), None);
        assert_eq!(decode(Event::FocusGained), None);
    }
}
