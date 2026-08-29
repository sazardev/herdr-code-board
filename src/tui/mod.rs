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

use state::{App, Key, PickerTarget, RepoChoice, Request};
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

pub fn run(paths: &Paths, config: &Config) -> Result<()> {
    let herdr: Arc<dyn HerdrApi> = Arc::new(CliHerdr::new());
    let theme = Theme::from_config(&config.theme);

    let mut app = App::new(
        agents::KINDS.iter().map(|s| s.to_string()).collect(),
        config.default_agent.clone(),
    );
    reload(paths, &mut app)?;

    let mut terminal = ratatui::init();
    let result = event_loop(paths, config, herdr, &mut terminal, &mut app, &theme);
    ratatui::restore();
    result
}

fn reload(paths: &Paths, app: &mut App) -> Result<()> {
    let store = Store::open(&paths.database())?;
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
    let store = Store::open(&paths.database())?;
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
                if let Err(e) = execute(paths, config, herdr.clone(), app, request) {
                    app.status = format!("{e:#}");
                }
                reload(paths, app)?;
                revision = Store::open(&paths.database())?.revision()?;
                continue;
            }
        }

        // Cheap change detection: the engine and the hooks bump the board's
        // revision, so the TUI never has to read a pane to stay current.
        if last_poll.elapsed() >= poll {
            last_poll = Instant::now();
            let store = Store::open(&paths.database())?;
            let current = store.revision()?;
            if current != revision {
                revision = current;
                app.load(store.list_cards()?, store.list_repos()?);
            }
        }
    }
}

fn execute(
    paths: &Paths,
    config: &Config,
    herdr: Arc<dyn HerdrApi>,
    app: &mut App,
    request: Request,
) -> Result<()> {
    let store = Store::open(&paths.database())?;
    match request {
        Request::None | Request::Quit => {}
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
                match overlay::sync_repo(&store, &path, &config.default_agent) {
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
            let items: Vec<RepoChoice> = crate::app::scan(&store, config.roots())?
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
            overlay::sync_repo(&store, &path, &config.default_agent)?;
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
