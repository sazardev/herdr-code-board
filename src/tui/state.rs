//! Board view state and key handling.
//!
//! This module never touches the terminal, the database or herdr. It turns key
//! presses into [`Request`]s that the run loop executes, which keeps every
//! keybinding testable.

use std::path::PathBuf;

use crate::model::{Card, Column, Repo, Trigger};
use crate::store::cards::NewCard;

use super::form::{Field, Form};

/// Work the run loop must do on the app's behalf.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    None,
    Quit,
    Reload,
    Sync,
    SetLane {
        card_id: String,
        lane: Column,
    },
    Cancel {
        card_id: String,
    },
    Retry {
        card_id: String,
    },
    Delete {
        card_id: String,
    },
    FocusPane {
        pane_id: String,
    },
    Create(Box<NewCard>),
    Update(Box<Card>),
    /// Scan the disk and hand the results back through [`App::open_picker`].
    /// The app cannot do it itself: it performs no I/O.
    ScanRepos(PickerTarget),
    /// Track this checkout if it is not tracked, then apply it to `target`.
    UseRepo {
        path: PathBuf,
        target: PickerTarget,
    },
    /// The form needs this repo's branches for its `from` chooser.
    LoadBranches(PathBuf),

    /// One line of text becomes a queued card in the current repo. The fast path.
    QuickAdd(String),
    /// Open the card's prompt in `$EDITOR`. The run loop suspends the terminal.
    EditPrompt(String),
    /// Copy a card into the backlog, ready to tweak.
    Duplicate(String),
    /// Move a card up (-1) or down (+1) within its lane.
    Reorder {
        card_id: String,
        delta: i64,
    },
    /// Queue every card currently shown in a lane.
    QueueLane(Vec<String>),
    /// Read a card's rules, runs and log for the detail overlay.
    LoadDetail(String),
    /// Chain one card to another.
    Chain {
        from: String,
        to: String,
        trigger: Trigger,
    },
    DeleteRule(String),
}

/// What the repo picker is being opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerTarget {
    /// Set the board's repo filter.
    Filter,
    /// Set the repo of the card being written.
    Form,
}

/// What chaining offers. Timed triggers carry a duration people actually use;
/// anything else is a rule you write in the overlay file.
pub fn chain_triggers() -> Vec<(&'static str, Trigger)> {
    vec![
        ("when it is done", Trigger::Done),
        ("when it fails", Trigger::Failed),
        ("when it is blocked", Trigger::Blocked),
        ("when it goes to review", Trigger::Review),
        ("after waiting 5m", Trigger::WaitingFor { seconds: 300 }),
        ("after waiting 15m", Trigger::WaitingFor { seconds: 900 }),
        ("after waiting 1h", Trigger::WaitingFor { seconds: 3_600 }),
        ("after 5m blocked", Trigger::BlockedFor { seconds: 300 }),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainStage {
    PickCard,
    PickTrigger,
}

/// Connecting one card to another: choose the follower, then the condition.
#[derive(Debug, Clone)]
pub struct Chain {
    pub from: String,
    pub from_title: String,
    pub stage: ChainStage,
    pub query: String,
    pub cursor: usize,
    pub candidates: Vec<(String, String)>,
    pub chosen: Option<(String, String)>,
    pub trigger: usize,
}

impl Chain {
    pub fn matches(&self) -> Vec<&(String, String)> {
        if self.query.is_empty() {
            return self.candidates.iter().collect();
        }
        let needle = self.query.to_lowercase();
        self.candidates
            .iter()
            .filter(|(_, title)| title.to_lowercase().contains(&needle))
            .collect()
    }
}

/// A card's rules, history and log, loaded on demand for the detail overlay.
#[derive(Debug, Clone, Default)]
pub struct Detail {
    pub card_id: String,
    pub title: String,
    pub prompt: String,
    /// `(rule id, one-line description)`.
    pub rules: Vec<(String, String)>,
    pub runs: Vec<String>,
    pub events: Vec<String>,
    pub cursor: usize,
}

/// One row of the repo picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoChoice {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub tracked: bool,
}

/// A searchable list of checkouts found on disk.
#[derive(Debug, Clone)]
pub struct Picker {
    pub items: Vec<RepoChoice>,
    pub query: String,
    pub cursor: usize,
    pub target: PickerTarget,
}

impl Picker {
    pub fn new(items: Vec<RepoChoice>, target: PickerTarget) -> Self {
        Self {
            items,
            query: String::new(),
            cursor: 0,
            target,
        }
    }

    /// Subsequence match, so `hcb` finds `herdr-code-board`.
    fn fuzzy(haystack: &str, needle: &str) -> bool {
        let mut chars = haystack.chars().flat_map(char::to_lowercase);
        needle
            .chars()
            .flat_map(char::to_lowercase)
            .all(|want| chars.any(|c| c == want))
    }

    pub fn matches(&self) -> Vec<&RepoChoice> {
        if self.query.is_empty() {
            return self.items.iter().collect();
        }
        self.items
            .iter()
            .filter(|i| {
                Self::fuzzy(&i.name, &self.query)
                    || i.path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&self.query.to_lowercase())
            })
            .collect()
    }

    pub fn selected(&self) -> Option<&RepoChoice> {
        self.matches().get(self.cursor).copied()
    }

    fn clamp(&mut self) {
        let len = self.matches().len();
        self.cursor = self.cursor.min(len.saturating_sub(1));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    Help,
    Form,
    /// Choosing a repository from what is on disk.
    RepoPicker,
    /// One-line capture: type a prompt, press enter, it runs.
    QuickAdd,
    /// Connecting this card to another one.
    Chain,
    /// Everything known about a card: rules, runs, log.
    Detail,
    /// A yes/no gate in front of something destructive.
    Confirm(String),
}

/// One key press, already decoded from the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Tab,
    BackTab,
    Left,
    Right,
    Up,
    Down,
}

pub struct App {
    pub cards: Vec<Card>,
    pub repos: Vec<Repo>,
    pub agents: Vec<String>,
    pub default_agent: String,
    pub mode: Mode,
    pub form: Option<Form>,
    pub picker: Option<Picker>,
    pub chain: Option<Chain>,
    pub detail: Option<Detail>,
    /// Buffer for the one-line quick add.
    pub quick: String,
    /// Started as the capture popup: leave as soon as the line is dealt with.
    pub oneshot: bool,
    /// Set while a scan is in flight, so the board can say so.
    pub scanning: bool,
    /// Selected lane, as an index into [`Column::ALL`].
    pub lane: usize,
    /// Selected card within each lane.
    pub cursor: [usize; Column::ALL.len()],
    pub search: String,
    /// `None` means every repo; `Some(i)` indexes `repos`.
    pub repo_filter: Option<usize>,
    pub status: String,
    /// Pending destructive request, released when the confirmation is accepted.
    pending: Option<Request>,
}

impl App {
    pub fn new(agents: Vec<String>, default_agent: String) -> Self {
        Self {
            cards: Vec::new(),
            repos: Vec::new(),
            agents,
            default_agent,
            mode: Mode::Normal,
            form: None,
            picker: None,
            chain: None,
            detail: None,
            quick: String::new(),
            oneshot: false,
            scanning: false,
            lane: 0,
            cursor: [0; Column::ALL.len()],
            search: String::new(),
            repo_filter: None,
            status: String::new(),
            pending: None,
        }
    }

    /// Open straight into one-line capture and close once it is submitted.
    pub fn start_quick(&mut self) {
        self.oneshot = true;
        self.mode = Mode::QuickAdd;
        self.quick.clear();
    }

    pub fn load(&mut self, cards: Vec<Card>, repos: Vec<Repo>) {
        self.cards = cards;
        self.repos = repos;
        if let Some(i) = self.repo_filter {
            if i >= self.repos.len() {
                self.repo_filter = None;
            }
        }
        self.clamp();
    }

    /// Show the picker with freshly scanned results.
    pub fn open_picker(&mut self, items: Vec<RepoChoice>, target: PickerTarget) {
        self.scanning = false;
        self.status.clear();
        if items.is_empty() {
            self.status = "no checkouts found; set scan_roots in config.toml".into();
            return;
        }
        self.picker = Some(Picker::new(items, target));
        self.mode = Mode::RepoPicker;
    }

    /// Give the form its `from` choices once the run loop has read them.
    pub fn set_branches(&mut self, branches: Vec<String>) {
        if let Some(form) = self.form.as_mut() {
            form.set_bases(branches);
        }
    }

    fn matches(&self, card: &Card) -> bool {
        if let Some(i) = self.repo_filter {
            let Some(repo) = self.repos.get(i) else {
                return true;
            };
            if card.repo_id.as_deref() != Some(repo.id.as_str()) {
                return false;
            }
        }
        if self.search.is_empty() {
            return true;
        }
        let needle = self.search.to_lowercase();
        card.title.to_lowercase().contains(&needle)
            || card.prompt.to_lowercase().contains(&needle)
            || card.tags.iter().any(|t| t.to_lowercase().contains(&needle))
    }

    /// Cards of one lane, after the active filters.
    pub fn lane_cards(&self, lane: Column) -> Vec<&Card> {
        self.cards
            .iter()
            .filter(|c| c.column == lane && self.matches(c))
            .collect()
    }

    pub fn current_lane(&self) -> Column {
        Column::ALL[self.lane.min(Column::ALL.len() - 1)]
    }

    pub fn selected(&self) -> Option<&Card> {
        let lane = self.current_lane();
        let cards = self.lane_cards(lane);
        cards.get(self.cursor[self.lane]).copied()
    }

    pub fn repo_name(&self, card: &Card) -> &str {
        card.repo_id
            .as_ref()
            .and_then(|id| self.repos.iter().find(|r| &r.id == id))
            .map(|r| r.name.as_str())
            .unwrap_or("-")
    }

    pub fn filter_label(&self) -> String {
        match self.repo_filter.and_then(|i| self.repos.get(i)) {
            Some(r) => r.name.clone(),
            None => "all repos".into(),
        }
    }

    fn clamp(&mut self) {
        self.lane = self.lane.min(Column::ALL.len() - 1);
        for (i, lane) in Column::ALL.iter().enumerate() {
            let len = self.lane_cards(*lane).len();
            self.cursor[i] = self.cursor[i].min(len.saturating_sub(1));
        }
    }

    fn move_lane(&mut self, delta: isize) {
        let len = Column::ALL.len() as isize;
        self.lane = (((self.lane as isize + delta) % len + len) % len) as usize;
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = self.lane_cards(self.current_lane()).len() as isize;
        if len == 0 {
            self.cursor[self.lane] = 0;
            return;
        }
        let next = (self.cursor[self.lane] as isize + delta).clamp(0, len - 1);
        self.cursor[self.lane] = next as usize;
    }

    /// Shift the selected card one lane over.
    fn shift_card(&mut self, delta: isize) -> Request {
        let Some(card) = self.selected().cloned() else {
            return Request::None;
        };
        let idx = Column::ALL
            .iter()
            .position(|c| *c == card.column)
            .unwrap_or(0) as isize;
        let len = Column::ALL.len() as isize;
        let target = Column::ALL[(((idx + delta) % len + len) % len) as usize];
        if card.column.is_live() && !target.is_live() {
            self.status = format!("{} is running; use x to cancel it instead", card.title);
            return Request::None;
        }
        self.move_lane(delta);
        Request::SetLane {
            card_id: card.id,
            lane: target,
        }
    }

    /// Nudge the selected card up or down its lane, following it with the cursor.
    fn reorder(&mut self, delta: i64) -> Request {
        let Some(card) = self.selected().cloned() else {
            return Request::None;
        };
        let len = self.lane_cards(self.current_lane()).len();
        if len < 2 {
            return Request::None;
        }
        let next = (self.cursor[self.lane] as i64 + delta).clamp(0, len as i64 - 1);
        self.cursor[self.lane] = next as usize;
        Request::Reorder {
            card_id: card.id,
            delta,
        }
    }

    pub fn on_key(&mut self, key: Key) -> Request {
        match self.mode.clone() {
            Mode::Normal => self.key_normal(key),
            Mode::Search => self.key_search(key),
            Mode::Help => {
                self.mode = Mode::Normal;
                Request::None
            }
            Mode::Confirm(_) => self.key_confirm(key),
            Mode::Form => self.key_form(key),
            Mode::RepoPicker => self.key_picker(key),
            Mode::QuickAdd => self.key_quick(key),
            Mode::Chain => self.key_chain(key),
            Mode::Detail => self.key_detail(key),
        }
    }

    fn key_normal(&mut self, key: Key) -> Request {
        self.status.clear();
        match key {
            Key::Char('q') | Key::Esc => Request::Quit,
            Key::Char('h') | Key::Left => {
                self.move_lane(-1);
                Request::None
            }
            Key::Char('l') | Key::Right => {
                self.move_lane(1);
                Request::None
            }
            Key::Char('j') | Key::Down => {
                self.move_cursor(1);
                Request::None
            }
            Key::Char('k') | Key::Up => {
                self.move_cursor(-1);
                Request::None
            }
            // Jump straight to a lane. Faster than walking there.
            Key::Char(c @ '1'..='9') => {
                let idx = c as usize - '1' as usize;
                if idx < Column::ALL.len() {
                    self.lane = idx;
                }
                Request::None
            }
            Key::Char('g') => {
                self.cursor[self.lane] = 0;
                Request::None
            }
            Key::Char('G') => {
                self.move_cursor(isize::MAX / 2);
                Request::None
            }
            Key::Char('H') => self.shift_card(-1),
            Key::Char('L') => self.shift_card(1),
            Key::Char('K') => self.reorder(-1),
            Key::Char('J') => self.reorder(1),

            // Capture: one line, queued immediately.
            Key::Char('a') => {
                self.quick.clear();
                self.mode = Mode::QuickAdd;
                Request::None
            }
            Key::Char('y') => match self.selected() {
                Some(card) => Request::Duplicate(card.id.clone()),
                None => Request::None,
            },
            Key::Char('E') => match self.selected() {
                Some(card) => Request::EditPrompt(card.id.clone()),
                None => Request::None,
            },
            Key::Char('v') => match self.selected() {
                Some(card) => Request::LoadDetail(card.id.clone()),
                None => Request::None,
            },
            Key::Char('c') => match self.selected().cloned() {
                Some(card) => {
                    let candidates: Vec<(String, String)> = self
                        .cards
                        .iter()
                        .filter(|c| c.id != card.id)
                        .map(|c| (c.id.clone(), c.title.clone()))
                        .collect();
                    if candidates.is_empty() {
                        self.status = "add a second card first, then chain them".into();
                        return Request::None;
                    }
                    self.chain = Some(Chain {
                        from: card.id.clone(),
                        from_title: card.title.clone(),
                        stage: ChainStage::PickCard,
                        query: String::new(),
                        cursor: 0,
                        candidates,
                        chosen: None,
                        trigger: 0,
                    });
                    self.mode = Mode::Chain;
                    Request::None
                }
                None => Request::None,
            },
            Key::Char('Q') => {
                let lane = self.current_lane();
                if lane.is_live() || lane == Column::Ready {
                    self.status = format!("{lane} is already moving");
                    return Request::None;
                }
                let ids: Vec<String> = self
                    .lane_cards(lane)
                    .into_iter()
                    .map(|c| c.id.clone())
                    .collect();
                if ids.is_empty() {
                    Request::None
                } else {
                    Request::QueueLane(ids)
                }
            }
            Key::Char(' ') => match self.selected().cloned() {
                Some(card) if card.column == Column::Ready => Request::SetLane {
                    card_id: card.id,
                    lane: Column::Backlog,
                },
                Some(card) if !card.column.is_live() => Request::SetLane {
                    card_id: card.id,
                    lane: Column::Ready,
                },
                Some(card) => {
                    self.status = format!("{} is already live", card.title);
                    Request::None
                }
                None => Request::None,
            },
            Key::Enter => match self.selected().and_then(|c| c.binding.pane_id.clone()) {
                Some(pane_id) => Request::FocusPane { pane_id },
                None => {
                    self.status = "that card owns no pane yet".into();
                    Request::None
                }
            },
            Key::Char('x') => match self.selected().cloned() {
                Some(card) => Request::Cancel { card_id: card.id },
                None => Request::None,
            },
            Key::Char('r') => match self.selected().cloned() {
                Some(card) => Request::Retry { card_id: card.id },
                None => Request::None,
            },
            Key::Char('d') => match self.selected().cloned() {
                Some(card) => {
                    self.pending = Some(Request::Delete {
                        card_id: card.id.clone(),
                    });
                    self.mode = Mode::Confirm(format!("delete {:?}?", card.title));
                    Request::None
                }
                None => Request::None,
            },
            Key::Char('n') => {
                let mut form = Form::new(&self.repos, self.agents.clone(), &self.default_agent);
                // Standing in a repo, or filtered to one? That is the one you mean.
                if let Some(i) = self.repo_filter {
                    if let Some(repo) = self.repos.get(i) {
                        if let Some(idx) = form.repos.iter().position(|(_, id)| *id == repo.id) {
                            form.repo = idx;
                        }
                    }
                }
                self.form = Some(form);
                self.mode = Mode::Form;
                // Nothing tracked yet: go straight to picking a repo instead of
                // presenting a form whose repo field has nothing in it.
                if self.repos.is_empty() {
                    self.scanning = true;
                    return Request::ScanRepos(PickerTarget::Form);
                }
                self.form_branch_request()
            }
            Key::Char('e') => match self.selected() {
                Some(card) => {
                    self.form = Some(Form::edit(card, &self.repos, self.agents.clone()));
                    self.mode = Mode::Form;
                    self.form_branch_request()
                }
                None => Request::None,
            },
            Key::Char('/') => {
                self.mode = Mode::Search;
                self.search.clear();
                Request::None
            }
            Key::Tab => {
                self.repo_filter = match self.repo_filter {
                    None if self.repos.is_empty() => None,
                    None => Some(0),
                    Some(i) if i + 1 < self.repos.len() => Some(i + 1),
                    Some(_) => None,
                };
                self.clamp();
                Request::None
            }
            Key::Char('t') => {
                self.scanning = true;
                self.status = "scanning for repositories…".into();
                Request::ScanRepos(PickerTarget::Filter)
            }
            Key::Char('s') => Request::Sync,
            Key::Char('R') => Request::Reload,
            Key::Char('?') => {
                self.mode = Mode::Help;
                Request::None
            }
            _ => Request::None,
        }
    }

    fn key_search(&mut self, key: Key) -> Request {
        match key {
            Key::Esc => {
                self.search.clear();
                self.mode = Mode::Normal;
            }
            Key::Enter => self.mode = Mode::Normal,
            Key::Backspace => {
                self.search.pop();
            }
            Key::Char(c) => self.search.push(c),
            _ => {}
        }
        self.clamp();
        Request::None
    }

    fn key_picker(&mut self, key: Key) -> Request {
        let Some(picker) = self.picker.as_mut() else {
            self.mode = Mode::Normal;
            return Request::None;
        };
        match key {
            Key::Esc => {
                self.picker = None;
                // Cancelling out of the form's picker returns to the form.
                self.mode = if self.form.is_some() {
                    Mode::Form
                } else {
                    Mode::Normal
                };
                Request::None
            }
            Key::Down | Key::Tab => {
                let len = picker.matches().len();
                if len > 0 {
                    picker.cursor = (picker.cursor + 1) % len;
                }
                Request::None
            }
            Key::Up | Key::BackTab => {
                let len = picker.matches().len();
                if len > 0 {
                    picker.cursor = (picker.cursor + len - 1) % len;
                }
                Request::None
            }
            Key::Backspace => {
                picker.query.pop();
                picker.clamp();
                Request::None
            }
            Key::Char(c) => {
                picker.query.push(c);
                picker.cursor = 0;
                Request::None
            }
            Key::Enter => match picker.selected().cloned() {
                Some(choice) => {
                    let target = picker.target;
                    self.picker = None;
                    self.mode = if target == PickerTarget::Form {
                        Mode::Form
                    } else {
                        Mode::Normal
                    };
                    Request::UseRepo {
                        path: choice.path,
                        target,
                    }
                }
                None => Request::None,
            },
            _ => Request::None,
        }
    }

    /// Ask the run loop for the branches of whichever repo the form is on, so the
    /// `from` chooser is populated before the user reaches it.
    fn form_branch_request(&self) -> Request {
        let Some(form) = self.form.as_ref() else {
            return Request::None;
        };
        let Some(id) = form.repo_id() else {
            return Request::None;
        };
        match self.repos.iter().find(|r| r.id == id) {
            Some(repo) => Request::LoadBranches(PathBuf::from(&repo.path)),
            None => Request::None,
        }
    }

    /// Point the board's filter at a repo, by path.
    pub fn filter_by_path(&mut self, path: &std::path::Path) {
        self.repo_filter = self
            .repos
            .iter()
            .position(|r| std::path::Path::new(&r.path) == path);
        self.clamp();
    }

    /// Point the form at a repo, by path.
    pub fn form_repo_by_path(&mut self, path: &std::path::Path) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let id = self
            .repos
            .iter()
            .find(|r| std::path::Path::new(&r.path) == path)
            .map(|r| r.id.clone());
        if let Some(id) = id {
            if let Some(idx) = form.repos.iter().position(|(_, rid)| *rid == id) {
                form.repo = idx;
            }
        }
    }

    fn key_quick(&mut self, key: Key) -> Request {
        match key {
            Key::Esc => {
                self.quick.clear();
                self.mode = Mode::Normal;
                Request::None
            }
            Key::Backspace => {
                self.quick.pop();
                Request::None
            }
            Key::Char(c) => {
                self.quick.push(c);
                Request::None
            }
            Key::Enter => {
                let text = self.quick.trim().to_string();
                self.mode = Mode::Normal;
                self.quick.clear();
                if text.is_empty() {
                    Request::None
                } else {
                    Request::QuickAdd(text)
                }
            }
            _ => Request::None,
        }
    }

    fn key_chain(&mut self, key: Key) -> Request {
        let Some(chain) = self.chain.as_mut() else {
            self.mode = Mode::Normal;
            return Request::None;
        };
        match chain.stage {
            ChainStage::PickCard => match key {
                Key::Esc => {
                    self.chain = None;
                    self.mode = Mode::Normal;
                    Request::None
                }
                Key::Down | Key::Tab => {
                    let len = chain.matches().len();
                    if len > 0 {
                        chain.cursor = (chain.cursor + 1) % len;
                    }
                    Request::None
                }
                Key::Up | Key::BackTab => {
                    let len = chain.matches().len();
                    if len > 0 {
                        chain.cursor = (chain.cursor + len - 1) % len;
                    }
                    Request::None
                }
                Key::Backspace => {
                    chain.query.pop();
                    chain.cursor = 0;
                    Request::None
                }
                Key::Char(c) => {
                    chain.query.push(c);
                    chain.cursor = 0;
                    Request::None
                }
                Key::Enter => {
                    if let Some(pick) = chain.matches().get(chain.cursor).map(|p| (*p).clone()) {
                        chain.chosen = Some(pick);
                        chain.stage = ChainStage::PickTrigger;
                        chain.cursor = 0;
                    }
                    Request::None
                }
                _ => Request::None,
            },
            ChainStage::PickTrigger => match key {
                // Back to the card list rather than losing the whole thing.
                Key::Esc => {
                    chain.stage = ChainStage::PickCard;
                    chain.chosen = None;
                    Request::None
                }
                Key::Down | Key::Tab => {
                    chain.trigger = (chain.trigger + 1) % chain_triggers().len();
                    Request::None
                }
                Key::Up | Key::BackTab => {
                    let len = chain_triggers().len();
                    chain.trigger = (chain.trigger + len - 1) % len;
                    Request::None
                }
                Key::Enter => {
                    let triggers = chain_triggers();
                    let trigger = triggers[chain.trigger.min(triggers.len() - 1)].1.clone();
                    let from = chain.from.clone();
                    let to = chain.chosen.as_ref().map(|(id, _)| id.clone());
                    self.chain = None;
                    self.mode = Mode::Normal;
                    match to {
                        Some(to) => Request::Chain { from, to, trigger },
                        None => Request::None,
                    }
                }
                _ => Request::None,
            },
        }
    }

    fn key_detail(&mut self, key: Key) -> Request {
        let Some(detail) = self.detail.as_mut() else {
            self.mode = Mode::Normal;
            return Request::None;
        };
        match key {
            Key::Esc | Key::Char('q') | Key::Char('v') => {
                self.detail = None;
                self.mode = Mode::Normal;
                Request::None
            }
            Key::Down | Key::Char('j') => {
                if !detail.rules.is_empty() {
                    detail.cursor = (detail.cursor + 1) % detail.rules.len();
                }
                Request::None
            }
            Key::Up | Key::Char('k') => {
                if !detail.rules.is_empty() {
                    let len = detail.rules.len();
                    detail.cursor = (detail.cursor + len - 1) % len;
                }
                Request::None
            }
            // Remove the highlighted rule.
            Key::Char('d') => match detail.rules.get(detail.cursor) {
                Some((id, _)) => Request::DeleteRule(id.clone()),
                None => Request::None,
            },
            Key::Char('E') => Request::EditPrompt(detail.card_id.clone()),
            _ => Request::None,
        }
    }

    /// Show a loaded detail overlay.
    pub fn open_detail(&mut self, detail: Detail) {
        self.detail = Some(detail);
        self.mode = Mode::Detail;
    }

    fn key_confirm(&mut self, key: Key) -> Request {
        match key {
            Key::Char('y') | Key::Char('Y') | Key::Enter => {
                self.mode = Mode::Normal;
                self.pending.take().unwrap_or(Request::None)
            }
            _ => {
                self.mode = Mode::Normal;
                self.pending = None;
                Request::None
            }
        }
    }

    fn key_form(&mut self, key: Key) -> Request {
        let Some(form) = self.form.as_mut() else {
            self.mode = Mode::Normal;
            return Request::None;
        };
        match key {
            Key::Esc => {
                self.mode = Mode::Normal;
                self.form = None;
                Request::None
            }
            Key::Tab | Key::Down => {
                form.next_field();
                Request::None
            }
            Key::BackTab | Key::Up => {
                form.prev_field();
                Request::None
            }
            Key::Left => {
                form.cycle(false);
                Request::None
            }
            Key::Right => {
                form.cycle(true);
                Request::None
            }
            Key::Backspace => {
                form.backspace();
                Request::None
            }
            Key::Char(' ') if !form.current().is_text() => {
                form.toggle();
                Request::None
            }
            Key::Char(c) => {
                form.push(c);
                Request::None
            }
            Key::Enter if form.current() == Field::Repo => {
                self.scanning = true;
                Request::ScanRepos(PickerTarget::Form)
            }
            Key::Enter => {
                let form = self.form.take().expect("form present");
                self.mode = Mode::Normal;
                match &form.editing {
                    Some(id) => match self.cards.iter().find(|c| &c.id == id).cloned() {
                        Some(mut card) => {
                            form.apply_to(&mut card);
                            Request::Update(Box::new(card))
                        }
                        None => {
                            self.status = "that card is gone".into();
                            Request::None
                        }
                    },
                    None => match form.to_new_card() {
                        Some(card) => Request::Create(Box::new(card)),
                        None => {
                            self.status = "a card needs a title".into();
                            self.form = Some(form);
                            self.mode = Mode::Form;
                            Request::None
                        }
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Binding, Placement};

    fn card(id: &str, title: &str, column: Column) -> Card {
        Card {
            id: id.into(),
            key: None,
            title: title.into(),
            prompt: String::new(),
            repo_id: None,
            tags: vec![],
            agent_kind: "claude".into(),
            model: None,
            extra_args: vec![],
            placement: Placement::default(),
            column,
            binding: Binding::default(),
            priority: 0,
            auto_complete: true,
            auto_answer: false,
            max_retries: 0,
            attempts: 0,
            created_at: 0,
            updated_at: 0,
            status_since: 0,
            dispatched_at: None,
            last_error: None,
            prompt_sent: false,
        }
    }

    fn app() -> App {
        let mut app = App::new(vec!["claude".into(), "codex".into()], "claude".into());
        app.load(
            vec![
                card("1", "alpha", Column::Backlog),
                card("2", "beta", Column::Backlog),
                card("3", "gamma", Column::Running),
            ],
            vec![],
        );
        app
    }

    #[test]
    fn navigation_wraps_across_lanes_and_clamps_within_one() {
        let mut a = app();
        assert_eq!(a.current_lane(), Column::Backlog);
        a.on_key(Key::Char('h'));
        assert_eq!(a.current_lane(), Column::Cancelled, "wraps backwards");
        a.on_key(Key::Char('l'));
        assert_eq!(a.current_lane(), Column::Backlog);

        a.on_key(Key::Char('j'));
        assert_eq!(a.selected().unwrap().title, "beta");
        a.on_key(Key::Char('j'));
        assert_eq!(a.selected().unwrap().title, "beta", "clamped at the end");
        a.on_key(Key::Char('k'));
        assert_eq!(a.selected().unwrap().title, "alpha");
    }

    #[test]
    fn space_queues_a_backlog_card_and_unqueues_a_ready_one() {
        let mut a = app();
        assert_eq!(
            a.on_key(Key::Char(' ')),
            Request::SetLane {
                card_id: "1".into(),
                lane: Column::Ready
            }
        );

        a.load(vec![card("1", "alpha", Column::Ready)], vec![]);
        a.lane = Column::ALL
            .iter()
            .position(|c| *c == Column::Ready)
            .unwrap();
        assert_eq!(
            a.on_key(Key::Char(' ')),
            Request::SetLane {
                card_id: "1".into(),
                lane: Column::Backlog
            }
        );
    }

    #[test]
    fn a_running_card_cannot_be_dragged_out_of_its_lane_by_accident() {
        let mut a = app();
        a.lane = Column::ALL
            .iter()
            .position(|c| *c == Column::Running)
            .unwrap();
        assert_eq!(a.selected().unwrap().title, "gamma");
        assert_eq!(a.on_key(Key::Char('H')), Request::None);
        assert!(a.status.contains("cancel"));
        // Cancelling is still one key away.
        assert_eq!(
            a.on_key(Key::Char('x')),
            Request::Cancel {
                card_id: "3".into()
            }
        );
    }

    #[test]
    fn shifting_a_backlog_card_moves_it_and_follows_it() {
        let mut a = app();
        assert_eq!(
            a.on_key(Key::Char('L')),
            Request::SetLane {
                card_id: "1".into(),
                lane: Column::Ready
            }
        );
        assert_eq!(a.current_lane(), Column::Ready, "the view follows the card");
    }

    #[test]
    fn deleting_asks_first() {
        let mut a = app();
        assert_eq!(a.on_key(Key::Char('d')), Request::None);
        assert!(matches!(a.mode, Mode::Confirm(_)));

        // Anything but yes cancels.
        assert_eq!(a.on_key(Key::Char('n')), Request::None);
        assert_eq!(a.mode, Mode::Normal);

        a.on_key(Key::Char('d'));
        assert_eq!(
            a.on_key(Key::Char('y')),
            Request::Delete {
                card_id: "1".into()
            }
        );
    }

    #[test]
    fn enter_jumps_to_the_pane_only_when_there_is_one() {
        let mut a = app();
        assert_eq!(a.on_key(Key::Enter), Request::None);
        assert!(a.status.contains("no pane"));

        let mut running = card("3", "gamma", Column::Running);
        running.binding = Binding {
            pane_id: Some("w1:p4".into()),
            ..Default::default()
        };
        a.load(vec![running], vec![]);
        a.lane = Column::ALL
            .iter()
            .position(|c| *c == Column::Running)
            .unwrap();
        assert_eq!(
            a.on_key(Key::Enter),
            Request::FocusPane {
                pane_id: "w1:p4".into()
            }
        );
    }

    #[test]
    fn search_filters_by_title_and_is_cleared_by_escape() {
        let mut a = app();
        a.on_key(Key::Char('/'));
        for c in "bet".chars() {
            a.on_key(Key::Char(c));
        }
        assert_eq!(a.lane_cards(Column::Backlog).len(), 1);
        a.on_key(Key::Enter);
        assert_eq!(a.mode, Mode::Normal);

        a.on_key(Key::Char('/'));
        a.on_key(Key::Esc);
        assert_eq!(a.lane_cards(Column::Backlog).len(), 2);
    }

    #[test]
    fn the_repo_filter_cycles_through_repos_and_back_to_all() {
        let mut a = app();
        let repo = Repo {
            id: "R1".into(),
            name: "erp".into(),
            path: "/erp".into(),
            tags: vec![],
            max_parallel: 2,
            default_agent: None,
            default_model: None,
        };
        let mut owned = card("4", "delta", Column::Backlog);
        owned.repo_id = Some("R1".into());
        a.load(vec![card("1", "alpha", Column::Backlog), owned], vec![repo]);

        assert_eq!(a.filter_label(), "all repos");
        a.on_key(Key::Tab);
        assert_eq!(a.filter_label(), "erp");
        assert_eq!(a.lane_cards(Column::Backlog).len(), 1);
        a.on_key(Key::Tab);
        assert_eq!(a.filter_label(), "all repos");
        assert_eq!(a.lane_cards(Column::Backlog).len(), 2);
    }

    #[test]
    fn the_form_creates_a_card_and_escape_throws_it_away() {
        let mut a = app();
        a.on_key(Key::Char('n'));
        assert_eq!(a.mode, Mode::Form);
        for c in "hello".chars() {
            a.on_key(Key::Char(c));
        }
        match a.on_key(Key::Enter) {
            Request::Create(card) => assert_eq!(card.title, "hello"),
            other => panic!("expected Create, got {other:?}"),
        }

        a.on_key(Key::Char('n'));
        a.on_key(Key::Char('x'));
        assert_eq!(a.on_key(Key::Esc), Request::None);
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.form.is_none());
    }

    #[test]
    fn submitting_an_untitled_form_keeps_it_open_and_says_why() {
        let mut a = app();
        a.on_key(Key::Char('n'));
        assert_eq!(a.on_key(Key::Enter), Request::None);
        assert_eq!(
            a.mode,
            Mode::Form,
            "the form must not vanish with the input"
        );
        assert!(a.status.contains("title"));
    }

    fn choice(name: &str, path: &str, tracked: bool) -> RepoChoice {
        RepoChoice {
            name: name.into(),
            path: PathBuf::from(path),
            branch: Some("main".into()),
            tracked,
        }
    }

    #[test]
    fn t_asks_for_a_scan_rather_than_blocking_on_one() {
        let mut a = app();
        assert_eq!(
            a.on_key(Key::Char('t')),
            Request::ScanRepos(PickerTarget::Filter)
        );
        assert!(a.scanning, "the board should say it is working");
        assert_eq!(a.mode, Mode::Normal, "the picker opens when results arrive");
    }

    #[test]
    fn the_picker_filters_by_fuzzy_subsequence() {
        let mut a = app();
        a.open_picker(
            vec![
                choice("herdr-code-board", "/h/Documents/herdr-code-board", false),
                choice("rustock", "/h/Documents/rustock", true),
            ],
            PickerTarget::Filter,
        );
        assert_eq!(a.mode, Mode::RepoPicker);

        for c in "hcb".chars() {
            a.on_key(Key::Char(c));
        }
        let picker = a.picker.as_ref().unwrap();
        assert_eq!(picker.matches().len(), 1);
        assert_eq!(picker.matches()[0].name, "herdr-code-board");
    }

    #[test]
    fn the_picker_also_matches_on_path() {
        let mut a = app();
        a.open_picker(
            vec![
                choice("alpha", "/h/work/alpha", false),
                choice("beta", "/h/play/beta", false),
            ],
            PickerTarget::Filter,
        );
        for c in "work".chars() {
            a.on_key(Key::Char(c));
        }
        assert_eq!(a.picker.as_ref().unwrap().matches().len(), 1);
    }

    #[test]
    fn picking_a_repo_asks_the_loop_to_track_and_apply_it() {
        let mut a = app();
        a.open_picker(
            vec![choice("alpha", "/h/work/alpha", false)],
            PickerTarget::Filter,
        );
        assert_eq!(
            a.on_key(Key::Enter),
            Request::UseRepo {
                path: PathBuf::from("/h/work/alpha"),
                target: PickerTarget::Filter
            }
        );
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.picker.is_none());
    }

    #[test]
    fn escaping_the_pickers_returns_to_wherever_you_came_from() {
        // Opened from the board.
        let mut a = app();
        a.open_picker(vec![choice("a", "/a", false)], PickerTarget::Filter);
        a.on_key(Key::Esc);
        assert_eq!(a.mode, Mode::Normal);

        // Opened from the form: the form is still there behind it.
        let mut a = app();
        a.on_key(Key::Char('n'));
        a.open_picker(vec![choice("a", "/a", false)], PickerTarget::Form);
        a.on_key(Key::Esc);
        assert_eq!(a.mode, Mode::Form);
        assert!(a.form.is_some());
    }

    #[test]
    fn an_empty_scan_says_so_instead_of_opening_a_blank_list() {
        let mut a = app();
        a.scanning = true;
        a.open_picker(vec![], PickerTarget::Filter);
        assert_eq!(a.mode, Mode::Normal);
        assert!(!a.scanning);
        assert!(a.status.contains("no checkouts"));
    }

    #[test]
    fn a_new_card_starts_in_whichever_repo_you_are_filtered_to() {
        let mut a = app();
        let repo = Repo {
            id: "R1".into(),
            name: "erp".into(),
            path: "/erp".into(),
            tags: vec![],
            max_parallel: 2,
            default_agent: None,
            default_model: None,
        };
        a.load(vec![], vec![repo]);
        a.on_key(Key::Tab); // filter to erp
        a.on_key(Key::Char('n'));

        assert_eq!(a.form.as_ref().unwrap().repo_id().as_deref(), Some("R1"));
    }

    #[test]
    fn a_new_card_with_nothing_tracked_goes_straight_to_the_picker() {
        let mut a = app();
        a.load(vec![], vec![]);
        assert_eq!(
            a.on_key(Key::Char('n')),
            Request::ScanRepos(PickerTarget::Form)
        );
        assert!(a.form.is_some(), "the form is waiting behind the picker");
    }

    #[test]
    fn opening_the_form_on_a_repo_asks_for_its_branches() {
        let mut a = app();
        let repo = Repo {
            id: "R1".into(),
            name: "erp".into(),
            path: "/erp".into(),
            tags: vec![],
            max_parallel: 2,
            default_agent: None,
            default_model: None,
        };
        a.load(vec![], vec![repo]);
        a.on_key(Key::Tab);
        assert_eq!(
            a.on_key(Key::Char('n')),
            Request::LoadBranches(PathBuf::from("/erp"))
        );
        a.set_branches(vec!["main".into(), "develop".into()]);
        assert_eq!(a.form.as_ref().unwrap().base_name(), Some("main"));
    }

    #[test]
    fn enter_on_the_form_repo_field_opens_the_picker_instead_of_saving() {
        let mut a = app();
        a.on_key(Key::Char('n'));
        let idx = {
            let f = a.form.as_ref().unwrap();
            f.fields().iter().position(|x| *x == Field::Repo).unwrap()
        };
        a.form.as_mut().unwrap().field = idx;
        assert_eq!(a.on_key(Key::Enter), Request::ScanRepos(PickerTarget::Form));
        assert!(a.form.is_some(), "the half-written card is not thrown away");
    }

    #[test]
    fn digits_jump_straight_to_a_lane() {
        let mut a = app();
        a.on_key(Key::Char('3'));
        assert_eq!(a.current_lane(), Column::Running);
        a.on_key(Key::Char('1'));
        assert_eq!(a.current_lane(), Column::Backlog);
    }

    #[test]
    fn quick_add_captures_a_line_and_queues_it() {
        let mut a = app();
        a.on_key(Key::Char('a'));
        assert_eq!(a.mode, Mode::QuickAdd);
        for c in "fix the lint".chars() {
            a.on_key(Key::Char(c));
        }
        a.on_key(Key::Backspace);
        assert_eq!(
            a.on_key(Key::Enter),
            Request::QuickAdd("fix the lin".into())
        );
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.quick.is_empty(), "the buffer is cleared for next time");
    }

    #[test]
    fn the_capture_popup_opens_in_quick_add_and_is_marked_one_shot() {
        let mut a = app();
        a.start_quick();
        assert_eq!(a.mode, Mode::QuickAdd);
        assert!(a.oneshot);
        for c in "hi".chars() {
            a.on_key(Key::Char(c));
        }
        assert_eq!(a.on_key(Key::Enter), Request::QuickAdd("hi".into()));
        assert_eq!(a.mode, Mode::Normal, "the loop closes it from here");
    }

    #[test]
    fn quick_add_ignores_an_empty_line_and_escape_throws_it_away() {
        let mut a = app();
        a.on_key(Key::Char('a'));
        assert_eq!(a.on_key(Key::Enter), Request::None);

        a.on_key(Key::Char('a'));
        a.on_key(Key::Char('x'));
        a.on_key(Key::Esc);
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.quick.is_empty());
    }

    #[test]
    fn chaining_picks_a_card_then_a_condition() {
        let mut a = app();
        a.on_key(Key::Char('c'));
        assert_eq!(a.mode, Mode::Chain);
        let chain = a.chain.as_ref().unwrap();
        assert_eq!(chain.from_title, "alpha");
        assert_eq!(chain.candidates.len(), 2, "itself is not a candidate");

        for c in "gam".chars() {
            a.on_key(Key::Char(c));
        }
        assert_eq!(a.chain.as_ref().unwrap().matches().len(), 1);
        a.on_key(Key::Enter);
        assert_eq!(a.chain.as_ref().unwrap().stage, ChainStage::PickTrigger);

        // Second entry in the trigger list is "when it fails".
        a.on_key(Key::Down);
        match a.on_key(Key::Enter) {
            Request::Chain { from, to, trigger } => {
                assert_eq!(from, "1");
                assert_eq!(to, "3");
                assert_eq!(trigger, Trigger::Failed);
            }
            other => panic!("expected Chain, got {other:?}"),
        }
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn escaping_the_trigger_step_goes_back_to_the_card_list() {
        let mut a = app();
        a.on_key(Key::Char('c'));
        a.on_key(Key::Enter);
        assert_eq!(a.chain.as_ref().unwrap().stage, ChainStage::PickTrigger);
        a.on_key(Key::Esc);
        assert_eq!(a.chain.as_ref().unwrap().stage, ChainStage::PickCard);
        assert_eq!(a.mode, Mode::Chain, "the whole thing is not abandoned");
        a.on_key(Key::Esc);
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn chaining_needs_something_to_chain_to() {
        let mut a = app();
        a.load(vec![card("1", "alone", Column::Backlog)], vec![]);
        assert_eq!(a.on_key(Key::Char('c')), Request::None);
        assert!(a.status.contains("second card"));
    }

    #[test]
    fn reordering_moves_the_cursor_with_the_card() {
        let mut a = app();
        assert_eq!(a.selected().unwrap().title, "alpha");
        assert_eq!(
            a.on_key(Key::Char('J')),
            Request::Reorder {
                card_id: "1".into(),
                delta: 1
            }
        );
        assert_eq!(a.cursor[a.lane], 1, "the cursor follows the card down");
    }

    #[test]
    fn reordering_a_lane_of_one_does_nothing() {
        let mut a = app();
        a.lane = Column::ALL
            .iter()
            .position(|c| *c == Column::Running)
            .unwrap();
        assert_eq!(a.on_key(Key::Char('J')), Request::None);
    }

    #[test]
    fn queueing_a_whole_lane_sends_every_visible_card() {
        let mut a = app();
        match a.on_key(Key::Char('Q')) {
            Request::QueueLane(ids) => assert_eq!(ids, vec!["1", "2"]),
            other => panic!("expected QueueLane, got {other:?}"),
        }

        // A search narrows what "the lane" means, which is the point.
        a.on_key(Key::Char('/'));
        for c in "alp".chars() {
            a.on_key(Key::Char(c));
        }
        a.on_key(Key::Enter);
        match a.on_key(Key::Char('Q')) {
            Request::QueueLane(ids) => assert_eq!(ids, vec!["1"]),
            other => panic!("expected QueueLane, got {other:?}"),
        }
    }

    #[test]
    fn a_live_lane_is_not_bulk_queued() {
        let mut a = app();
        a.lane = Column::ALL
            .iter()
            .position(|c| *c == Column::Running)
            .unwrap();
        assert_eq!(a.on_key(Key::Char('Q')), Request::None);
        assert!(a.status.contains("already moving"));
    }

    #[test]
    fn the_detail_overlay_walks_rules_and_removes_one() {
        let mut a = app();
        assert_eq!(a.on_key(Key::Char('v')), Request::LoadDetail("1".into()));

        a.open_detail(Detail {
            card_id: "1".into(),
            title: "alpha".into(),
            prompt: "p".into(),
            rules: vec![
                ("R1".into(), "when it is done → queue 1".into()),
                ("R2".into(), "when it fails → notify".into()),
            ],
            runs: vec![],
            events: vec![],
            cursor: 0,
        });
        assert_eq!(a.mode, Mode::Detail);
        a.on_key(Key::Char('j'));
        assert_eq!(a.on_key(Key::Char('d')), Request::DeleteRule("R2".into()));
        a.on_key(Key::Esc);
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn duplicating_and_editing_address_the_selected_card() {
        let mut a = app();
        assert_eq!(a.on_key(Key::Char('y')), Request::Duplicate("1".into()));
        assert_eq!(a.on_key(Key::Char('E')), Request::EditPrompt("1".into()));
    }

    #[test]
    fn help_closes_on_any_key() {
        let mut a = app();
        a.on_key(Key::Char('?'));
        assert_eq!(a.mode, Mode::Help);
        a.on_key(Key::Char('z'));
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn reloading_after_cards_disappear_does_not_leave_a_dangling_cursor() {
        let mut a = app();
        a.on_key(Key::Char('j'));
        a.load(vec![card("1", "alpha", Column::Backlog)], vec![]);
        assert_eq!(a.selected().unwrap().id, "1");
        a.load(vec![], vec![]);
        assert!(a.selected().is_none());
    }
}
