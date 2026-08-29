//! Domain types shared by the store, the engine and the TUI.

use std::fmt;
use std::str::FromStr;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// A kanban lane. The engine only ever moves a card between these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    /// Captured, not yet eligible for dispatch.
    Backlog,
    /// Eligible: the engine will dispatch it when a slot frees up.
    Ready,
    /// Agent started and actively working.
    Running,
    /// Agent went idle after our prompt; nobody has looked at it yet.
    Waiting,
    /// Herdr recognized an approval or question dialog.
    Blocked,
    /// Agent finished unseen work. Needs a human, unless `auto_complete`.
    Review,
    Done,
    Failed,
    Cancelled,
}

impl Column {
    /// Lane order as rendered left-to-right in the TUI.
    pub const ALL: [Column; 9] = [
        Column::Backlog,
        Column::Ready,
        Column::Running,
        Column::Waiting,
        Column::Blocked,
        Column::Review,
        Column::Done,
        Column::Failed,
        Column::Cancelled,
    ];

    /// A card in a live column owns a herdr pane and counts against concurrency.
    pub fn is_live(self) -> bool {
        matches!(self, Column::Running | Column::Waiting | Column::Blocked)
    }

    /// Terminal columns never transition again on their own.
    pub fn is_terminal(self) -> bool {
        matches!(self, Column::Done | Column::Failed | Column::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Column::Backlog => "backlog",
            Column::Ready => "ready",
            Column::Running => "running",
            Column::Waiting => "waiting",
            Column::Blocked => "blocked",
            Column::Review => "review",
            Column::Done => "done",
            Column::Failed => "failed",
            Column::Cancelled => "cancelled",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Column::Backlog => "Backlog",
            Column::Ready => "Ready",
            Column::Running => "Running",
            Column::Waiting => "Waiting",
            Column::Blocked => "Blocked",
            Column::Review => "Review",
            Column::Done => "Done",
            Column::Failed => "Failed",
            Column::Cancelled => "Cancelled",
        }
    }
}

impl fmt::Display for Column {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Column {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "backlog" => Column::Backlog,
            "ready" => Column::Ready,
            "running" => Column::Running,
            "waiting" => Column::Waiting,
            "blocked" => Column::Blocked,
            "review" => Column::Review,
            "done" => Column::Done,
            "failed" => Column::Failed,
            "cancelled" | "canceled" => Column::Cancelled,
            other => bail!("unknown column {other:?}"),
        })
    }
}

/// Herdr's agent lifecycle, mirrored from the socket API `AgentStatus` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl FromStr for AgentStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "idle" => AgentStatus::Idle,
            "working" => AgentStatus::Working,
            "blocked" => AgentStatus::Blocked,
            "done" => AgentStatus::Done,
            "unknown" => AgentStatus::Unknown,
            other => bail!("unknown agent status {other:?}"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Right,
    Down,
}

impl SplitDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            SplitDirection::Right => "right",
            SplitDirection::Down => "down",
        }
    }
}

/// Where the card's agent should land in the herdr layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Placement {
    /// Reuse an idle shell pane in the repo's workspace, splitting only if none is free.
    Reuse,
    /// Always split a new pane. This is the "vertical column per repo" default.
    Split {
        #[serde(default)]
        direction: Option<SplitDirection>,
        #[serde(default)]
        ratio: Option<f64>,
    },
    /// A fresh tab inside the repo's workspace.
    NewTab,
    /// A fresh workspace rooted at the repo.
    NewWorkspace,
    /// A git worktree, which herdr opens as its own workspace.
    Worktree {
        /// Branch name. `{card}` expands to the card slug.
        branch: String,
        #[serde(default)]
        base: Option<String>,
    },
}

impl Default for Placement {
    fn default() -> Self {
        Placement::Split {
            direction: None,
            ratio: None,
        }
    }
}

/// The live herdr objects a dispatched card owns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    pub pane_id: Option<String>,
    pub agent_name: Option<String>,
    /// Set when the engine created the worktree, so cleanup knows it owns it.
    pub worktree_path: Option<String>,
}

impl Binding {
    pub fn is_empty(&self) -> bool {
        self == &Binding::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub tags: Vec<String>,
    /// How many cards of this repo may be live at once.
    pub max_parallel: u32,
    /// Default agent kind for cards created against this repo.
    pub default_agent: Option<String>,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    /// Stable key for cards imported from a repo's `.herdr-board.toml`.
    pub key: Option<String>,
    pub title: String,
    pub prompt: String,
    pub repo_id: Option<String>,
    pub tags: Vec<String>,
    pub agent_kind: String,
    pub model: Option<String>,
    pub extra_args: Vec<String>,
    pub placement: Placement,
    pub column: Column,
    pub binding: Binding,
    pub priority: i64,
    /// Move straight to `done` instead of parking in `review`.
    pub auto_complete: bool,
    /// Answer `blocked` dialogs automatically. Also requires `allow_auto_answer` in config.
    pub auto_answer: bool,
    pub max_retries: u32,
    pub attempts: u32,
    pub created_at: i64,
    pub updated_at: i64,
    /// When the card entered its current column. Drives the `waiting_for` rules.
    pub status_since: i64,
    pub dispatched_at: Option<i64>,
    /// Last error the engine recorded for this card.
    pub last_error: Option<String>,
    /// True once a prompt has been delivered, so `idle` means "waiting" not "just started".
    pub prompt_sent: bool,
}

impl Card {
    /// Short, unique-ish identifier used for the herdr agent name and branch names.
    /// Herdr requires `[a-z][a-z0-9_-]{0,31}`.
    pub fn slug(&self) -> String {
        let mut out = String::new();
        for ch in self.title.chars() {
            let c = ch.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() {
                out.push(c);
            } else if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
            if out.len() >= 20 {
                break;
            }
        }
        let stem = out.trim_matches('-').to_string();
        // The id tail keeps names unique among live agents.
        let tail: String = self
            .id
            .chars()
            .rev()
            .take(6)
            .collect::<String>()
            .to_ascii_lowercase();
        let joined = if stem.is_empty() {
            format!("card-{tail}")
        } else {
            format!("{stem}-{tail}")
        };
        // Guarantee the leading-letter rule even for titles that start with a digit.
        if joined.starts_with(|c: char| c.is_ascii_alphabetic()) {
            joined
        } else {
            format!("c{joined}")
        }
    }
}

/// What makes a rule fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum Trigger {
    Done,
    Review,
    Failed,
    Blocked,
    /// The card has sat in `waiting` for at least this long.
    WaitingFor {
        seconds: i64,
    },
    /// The card has sat in `blocked` for at least this long.
    BlockedFor {
        seconds: i64,
    },
}

impl Trigger {
    /// How to say this trigger out loud, for `show`, `link` and the TUI.
    pub fn describe(&self) -> String {
        match self {
            Trigger::Done => "when it is done".into(),
            Trigger::Review => "when it goes to review".into(),
            Trigger::Failed => "when it fails".into(),
            Trigger::Blocked => "when it is blocked".into(),
            Trigger::WaitingFor { seconds } => {
                format!("after waiting {}", humanize_seconds(*seconds))
            }
            Trigger::BlockedFor { seconds } => {
                format!("after {} blocked", humanize_seconds(*seconds))
            }
        }
    }

    /// Deadline-based triggers need the timer wheel; the rest are event-driven.
    pub fn delay_seconds(&self) -> Option<i64> {
        match self {
            Trigger::WaitingFor { seconds } | Trigger::BlockedFor { seconds } => Some(*seconds),
            _ => None,
        }
    }

    /// The column a timed trigger watches.
    pub fn watched_column(&self) -> Option<Column> {
        match self {
            Trigger::WaitingFor { .. } => Some(Column::Waiting),
            Trigger::BlockedFor { .. } => Some(Column::Blocked),
            Trigger::Done => Some(Column::Done),
            Trigger::Review => Some(Column::Review),
            Trigger::Failed => Some(Column::Failed),
            Trigger::Blocked => Some(Column::Blocked),
        }
    }
}

/// What a rule does when it fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Action {
    /// Move the referenced cards to `ready` so the engine picks them up.
    Enqueue {
        cards: Vec<String>,
    },
    /// Send more text to the card's own agent.
    Prompt {
        text: String,
    },
    /// Answer a blocked dialog by picking the nth option (1-based).
    Answer {
        choice: u32,
    },
    /// Send raw herdr key combos to the card's agent.
    SendKeys {
        keys: Vec<String>,
    },
    Notify {
        title: String,
        #[serde(default)]
        body: Option<String>,
    },
    /// Re-dispatch this card from scratch.
    Retry,
    Cancel,
    ClosePane,
}

impl Action {
    /// How to say this action out loud.
    pub fn describe(&self) -> String {
        match self {
            Action::Enqueue { cards } => format!("queue {} card(s)", cards.len()),
            Action::Prompt { text } => format!("prompt {:?}", truncate(text, 40)),
            Action::Answer { choice } => format!("answer option {choice}"),
            Action::SendKeys { keys } => format!("send keys {}", keys.join(" ")),
            Action::Notify { title, .. } => format!("notify {title:?}"),
            Action::Retry => "re-dispatch it".into(),
            Action::Cancel => "cancel it".into(),
            Action::ClosePane => "close its pane".into(),
        }
    }

    /// Actions that write into a pane the user may be watching.
    pub fn touches_agent_input(&self) -> bool {
        matches!(
            self,
            Action::Prompt { .. } | Action::Answer { .. } | Action::SendKeys { .. }
        )
    }
}

/// `90s`, `15m`, `2h30m` — the inverse of the durations accepted in overlays.
pub fn humanize_seconds(total: i64) -> String {
    if total < 60 {
        return format!("{total}s");
    }
    let (h, m) = (total / 3_600, (total % 3_600) / 60);
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace('\n', " ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    /// Rules attach to a card, or to a repo (applying to every card of that repo).
    pub card_id: Option<String>,
    pub repo_id: Option<String>,
    pub trigger: Trigger,
    pub action: Action,
    /// How many times this rule may fire per card attempt. 0 means unlimited.
    pub max_fires: u32,
    pub fired: u32,
    pub enabled: bool,
}

/// One dispatch attempt of a card, for the audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub card_id: String,
    pub attempt: u32,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub outcome: Option<String>,
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, title: &str) -> Card {
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
            column: Column::Backlog,
            binding: Binding::default(),
            priority: 0,
            auto_complete: false,
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

    /// Herdr rejects agent names outside `[a-z][a-z0-9_-]{0,31}`.
    #[test]
    fn slug_matches_herdr_agent_name_rules() {
        let cases = [
            ("01JQ8ZK3ABCDEF", "Review the diff!"),
            ("01JQ8ZK3ABCDEF", "9 lives"),
            ("01JQ8ZK3ABCDEF", "..."),
            (
                "01JQ8ZK3ABCDEF",
                "A very long title that keeps going and going",
            ),
        ];
        for (id, title) in cases {
            let slug = card(id, title).slug();
            assert!(slug.len() <= 32, "{slug:?} too long");
            assert!(
                slug.starts_with(|c: char| c.is_ascii_lowercase()),
                "{slug:?} must start with a letter"
            );
            assert!(
                slug.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'),
                "{slug:?} has invalid characters"
            );
        }
    }

    #[test]
    fn slug_is_unique_per_card_id() {
        let a = card("01JQ8ZK3AAAAAA", "Review the diff").slug();
        let b = card("01JQ8ZK3BBBBBB", "Review the diff").slug();
        assert_ne!(a, b);
    }

    #[test]
    fn live_columns_hold_a_pane() {
        for c in Column::ALL {
            assert_eq!(
                c.is_live(),
                matches!(c, Column::Running | Column::Waiting | Column::Blocked)
            );
            assert!(!(c.is_live() && c.is_terminal()));
        }
    }

    #[test]
    fn durations_read_back_the_way_they_were_written() {
        assert_eq!(humanize_seconds(45), "45s");
        assert_eq!(humanize_seconds(900), "15m");
        assert_eq!(humanize_seconds(7_200), "2h");
        assert_eq!(humanize_seconds(5_400), "1h30m");
    }

    #[test]
    fn triggers_and_actions_describe_themselves_without_rust_syntax() {
        assert_eq!(Trigger::Done.describe(), "when it is done");
        assert_eq!(
            Trigger::WaitingFor { seconds: 900 }.describe(),
            "after waiting 15m"
        );
        assert_eq!(
            Action::Enqueue {
                cards: vec!["a".into(), "b".into()]
            }
            .describe(),
            "queue 2 card(s)"
        );
        assert_eq!(Action::Answer { choice: 1 }.describe(), "answer option 1");
        // A long multi-line prompt must not blow up a one-line listing.
        let long = Action::Prompt {
            text: "a".repeat(200) + "\nsecond line",
        }
        .describe();
        assert!(long.len() < 60, "{long}");
        assert!(!long.contains('\n'));
    }

    #[test]
    fn column_round_trips_through_str() {
        for c in Column::ALL {
            assert_eq!(c.as_str().parse::<Column>().unwrap(), c);
        }
    }
}
