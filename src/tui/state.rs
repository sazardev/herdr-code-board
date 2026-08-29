//! Board view state and key handling.
//!
//! This module never touches the terminal, the database or herdr. It turns key
//! presses into [`Request`]s that the run loop executes, which keeps every
//! keybinding testable.

use crate::model::{Card, Column, Repo};
use crate::store::cards::NewCard;

use super::form::Form;

/// Work the run loop must do on the app's behalf.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    None,
    Quit,
    Reload,
    Sync,
    SetLane { card_id: String, lane: Column },
    Cancel { card_id: String },
    Retry { card_id: String },
    Delete { card_id: String },
    FocusPane { pane_id: String },
    Create(Box<NewCard>),
    Update(Box<Card>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    Help,
    Form,
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
            lane: 0,
            cursor: [0; Column::ALL.len()],
            search: String::new(),
            repo_filter: None,
            status: String::new(),
            pending: None,
        }
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
                self.form = Some(Form::new(
                    &self.repos,
                    self.agents.clone(),
                    &self.default_agent,
                ));
                self.mode = Mode::Form;
                Request::None
            }
            Key::Char('e') => match self.selected() {
                Some(card) => {
                    self.form = Some(Form::edit(card, &self.repos, self.agents.clone()));
                    self.mode = Mode::Form;
                    Request::None
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
