//! `.herdr-board.toml`: cards a repository carries with it.
//!
//! The database is the board's source of truth, but the flows a team actually
//! shares — "review the diff, then run the tests, then open a PR" — belong in the
//! repo, in review, in git. An overlay file declares those as templates; `sync`
//! imports them idempotently, keyed by `(repo, key)`, and never touches a card's
//! live state.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{Action, Placement, Repo, SplitDirection, Trigger};
use crate::store::cards::NewCard;
use crate::store::Store;

pub const OVERLAY_FILE: &str = ".herdr-board.toml";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Overlay {
    #[serde(default)]
    pub repo: RepoSection,
    #[serde(default, rename = "card")]
    pub cards: Vec<CardSection>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepoSection {
    pub name: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub max_parallel: Option<u32>,
    pub agent: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CardSection {
    pub key: String,
    pub title: Option<String>,
    #[serde(default)]
    pub prompt: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// `reuse` | `split` | `tab` | `workspace` | `worktree`
    pub placement: Option<String>,
    pub direction: Option<String>,
    pub ratio: Option<f64>,
    pub branch: Option<String>,
    pub base: Option<String>,
    #[serde(default)]
    pub priority: i64,
    /// Park the card in `waiting` for a human instead of completing it.
    #[serde(default)]
    pub review: bool,
    #[serde(default)]
    pub auto_answer: bool,
    #[serde(default)]
    pub retries: u32,
    /// Import straight into `ready` so `sync` starts it.
    #[serde(default)]
    pub start: bool,
    #[serde(default, rename = "rules")]
    pub rules: Vec<RuleSection>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSection {
    /// `done` | `review` | `failed` | `blocked` | `waiting`
    pub on: String,
    /// Makes the trigger timed, e.g. `15m`. Valid with `waiting` and `blocked`.
    pub after: Option<String>,
    #[serde(default)]
    pub max_fires: u32,
    // Exactly one action below.
    #[serde(default)]
    pub enqueue: Vec<String>,
    pub prompt: Option<String>,
    pub answer: Option<u32>,
    #[serde(default)]
    pub keys: Vec<String>,
    pub notify: Option<String>,
    #[serde(default)]
    pub retry: bool,
    #[serde(default)]
    pub cancel: bool,
    #[serde(default)]
    pub close_pane: bool,
}

/// Parse a duration like `90s`, `15m`, `2h`, `1h30m`, or a bare number of seconds.
pub fn parse_duration(s: &str) -> Result<i64> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty duration");
    }
    if let Ok(n) = s.parse::<i64>() {
        return Ok(n);
    }

    let mut total = 0i64;
    let mut digits = String::new();
    let mut saw_unit = false;
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        if digits.is_empty() {
            bail!("{s:?} is not a duration; use forms like 90s, 15m, 2h, 1h30m");
        }
        let n: i64 = digits.parse()?;
        digits.clear();
        let mult = match ch {
            's' => 1,
            'm' => 60,
            'h' => 3_600,
            'd' => 86_400,
            other => bail!("unknown duration unit {other:?} in {s:?}"),
        };
        total += n * mult;
        saw_unit = true;
    }
    if !digits.is_empty() || !saw_unit {
        bail!("{s:?} is not a duration; use forms like 90s, 15m, 2h, 1h30m");
    }
    Ok(total)
}

impl CardSection {
    fn to_placement(&self) -> Result<Placement> {
        let direction = match self.direction.as_deref() {
            None => None,
            Some("right") => Some(SplitDirection::Right),
            Some("down") => Some(SplitDirection::Down),
            Some(other) => bail!("unknown direction {other:?}; use right or down"),
        };
        Ok(match self.placement.as_deref().unwrap_or("split") {
            "reuse" => Placement::Reuse,
            "split" => Placement::Split {
                direction,
                ratio: self.ratio,
            },
            "tab" => Placement::NewTab,
            "workspace" => Placement::NewWorkspace,
            "worktree" => Placement::Worktree {
                branch: self
                    .branch
                    .clone()
                    .unwrap_or_else(|| "board/{card}".to_string()),
                base: self.base.clone(),
            },
            other => {
                bail!("unknown placement {other:?}; use reuse, split, tab, workspace or worktree")
            }
        })
    }
}

impl RuleSection {
    fn to_trigger(&self) -> Result<Trigger> {
        let after = self.after.as_deref().map(parse_duration).transpose()?;
        Ok(match (self.on.as_str(), after) {
            ("done", None) => Trigger::Done,
            ("review", None) => Trigger::Review,
            ("failed", None) => Trigger::Failed,
            ("blocked", None) => Trigger::Blocked,
            ("blocked", Some(seconds)) => Trigger::BlockedFor { seconds },
            ("waiting", Some(seconds)) => Trigger::WaitingFor { seconds },
            ("waiting", None) => bail!("a `waiting` rule needs an `after` duration"),
            (other, Some(_)) => bail!("`after` is only valid on waiting or blocked, not {other:?}"),
            (other, None) => {
                bail!("unknown trigger {other:?}; use done, review, failed, blocked or waiting")
            }
        })
    }

    fn to_action(&self) -> Result<Action> {
        let mut found: Vec<Action> = Vec::new();
        if !self.enqueue.is_empty() {
            found.push(Action::Enqueue {
                cards: self.enqueue.clone(),
            });
        }
        if let Some(text) = &self.prompt {
            found.push(Action::Prompt { text: text.clone() });
        }
        if let Some(choice) = self.answer {
            found.push(Action::Answer { choice });
        }
        if !self.keys.is_empty() {
            found.push(Action::SendKeys {
                keys: self.keys.clone(),
            });
        }
        if let Some(title) = &self.notify {
            found.push(Action::Notify {
                title: title.clone(),
                body: None,
            });
        }
        if self.retry {
            found.push(Action::Retry);
        }
        if self.cancel {
            found.push(Action::Cancel);
        }
        if self.close_pane {
            found.push(Action::ClosePane);
        }

        match found.len() {
            1 => Ok(found.pop().unwrap()),
            0 => bail!("rule `on = {:?}` declares no action", self.on),
            _ => bail!(
                "rule `on = {:?}` declares {} actions; use one rule per action",
                self.on,
                found.len()
            ),
        }
    }
}

/// What a sync did, for reporting back to the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub repo_id: String,
    pub created: usize,
    pub updated: usize,
    pub rules: usize,
}

pub fn load(path: &Path) -> Result<Overlay> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

/// Import a repo's overlay file into the board.
///
/// Definition fields are refreshed on every sync; live state (lane, binding,
/// attempts) is never touched, so re-syncing while a card runs is safe.
pub fn sync_repo(store: &Store, repo_path: &Path, default_agent: &str) -> Result<SyncReport> {
    let file = repo_path.join(OVERLAY_FILE);
    let overlay = if file.exists() {
        load(&file)?
    } else {
        Overlay::default()
    };

    let name = overlay.repo.name.clone().unwrap_or_else(|| {
        repo_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| repo_path.display().to_string())
    });
    let existing = store.find_repo_by_path(&repo_path.to_string_lossy())?;
    let repo = store.upsert_repo(&Repo {
        id: String::new(),
        name,
        path: repo_path.to_string_lossy().to_string(),
        tags: if overlay.repo.tags.is_empty() {
            existing
                .as_ref()
                .map(|r| r.tags.clone())
                .unwrap_or_default()
        } else {
            overlay.repo.tags.clone()
        },
        max_parallel: overlay
            .repo
            .max_parallel
            .or(existing.as_ref().map(|r| r.max_parallel))
            .unwrap_or(2),
        default_agent: overlay
            .repo
            .agent
            .clone()
            .or_else(|| existing.as_ref().and_then(|r| r.default_agent.clone())),
        default_model: overlay
            .repo
            .model
            .clone()
            .or_else(|| existing.as_ref().and_then(|r| r.default_model.clone())),
    })?;

    let mut report = SyncReport {
        repo_id: repo.id.clone(),
        ..Default::default()
    };

    let agent_default = repo
        .default_agent
        .clone()
        .unwrap_or_else(|| default_agent.to_string());

    // Pass 1: cards, so rules can resolve `enqueue` keys to real ids.
    for section in &overlay.cards {
        let placement = section.to_placement()?;
        let title = section.title.clone().unwrap_or_else(|| section.key.clone());
        let agent_kind = section
            .agent
            .clone()
            .unwrap_or_else(|| agent_default.clone());
        let model = section.model.clone().or_else(|| repo.default_model.clone());

        match store.find_card_by_key(&repo.id, &section.key)? {
            Some(mut card) => {
                card.title = title;
                card.prompt = section.prompt.clone();
                card.agent_kind = agent_kind;
                card.model = model;
                card.extra_args = section.args.clone();
                card.tags = section.tags.clone();
                card.placement = placement;
                card.priority = section.priority;
                card.auto_complete = !section.review;
                card.auto_answer = section.auto_answer;
                card.max_retries = section.retries;
                store.update_card(&card)?;
                report.updated += 1;
            }
            None => {
                store.create_card(&NewCard {
                    key: Some(section.key.clone()),
                    title,
                    prompt: section.prompt.clone(),
                    repo_id: Some(repo.id.clone()),
                    // Overlay cards are definitions, not queued work: they are
                    // claimed by whichever session first runs them.
                    session: None,
                    tags: section.tags.clone(),
                    agent_kind,
                    model,
                    extra_args: section.args.clone(),
                    placement,
                    column: if section.start {
                        crate::model::Column::Ready
                    } else {
                        crate::model::Column::Backlog
                    },
                    priority: section.priority,
                    auto_complete: !section.review,
                    auto_answer: section.auto_answer,
                    max_retries: section.retries,
                })?;
                report.created += 1;
            }
        }
    }

    // Pass 2: rules. They are part of the definition, so they are replaced whole.
    for section in &overlay.cards {
        let Some(card) = store.find_card_by_key(&repo.id, &section.key)? else {
            continue;
        };
        store.delete_rules_for_card(&card.id)?;
        for rule in &section.rules {
            let trigger = rule.to_trigger()?;
            let mut action = rule.to_action()?;
            // Resolve sibling keys to ids so a key reused in another repo cannot
            // make the reference ambiguous later.
            if let Action::Enqueue { cards } = &mut action {
                for target in cards.iter_mut() {
                    if let Some(found) = store.find_card_by_key(&repo.id, target)? {
                        *target = found.id;
                    }
                }
            }
            store.add_rule(Some(&card.id), None, &trigger, &action, rule.max_fires)?;
            report.rules += 1;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Column;

    const SAMPLE: &str = r#"
[repo]
name = "erp"
tags = ["work"]
max_parallel = 3
agent = "claude"

[[card]]
key = "review-diff"
title = "Review the diff"
prompt = "Review the current diff and report only actionable findings."
start = true
priority = 10

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
"#;

    fn repo_dir(body: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(b) = body {
            std::fs::write(dir.path().join(OVERLAY_FILE), b).unwrap();
        }
        dir
    }

    #[test]
    fn durations_accept_the_forms_people_actually_type() {
        assert_eq!(parse_duration("90").unwrap(), 90);
        assert_eq!(parse_duration("90s").unwrap(), 90);
        assert_eq!(parse_duration("15m").unwrap(), 900);
        assert_eq!(parse_duration("2h").unwrap(), 7_200);
        assert_eq!(parse_duration("1h30m").unwrap(), 5_400);
        assert_eq!(parse_duration("1d").unwrap(), 86_400);
        for bad in ["", "soon", "15x", "m15", "15m30"] {
            assert!(parse_duration(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn a_repo_overlay_imports_cards_rules_and_repo_settings() {
        let store = Store::open_in_memory().unwrap();
        let dir = repo_dir(Some(SAMPLE));
        let report = sync_repo(&store, dir.path(), "codex").unwrap();

        assert_eq!(report.created, 2);
        assert_eq!(report.rules, 2);

        let repo = store.get_repo(&report.repo_id).unwrap().unwrap();
        assert_eq!(repo.name, "erp");
        assert_eq!(repo.max_parallel, 3);

        let review = store
            .find_card_by_key(&repo.id, "review-diff")
            .unwrap()
            .unwrap();
        assert_eq!(review.column, Column::Ready, "start = true");
        assert_eq!(review.agent_kind, "claude", "from the repo default");
        assert_eq!(review.priority, 10);

        let tests = store
            .find_card_by_key(&repo.id, "run-tests")
            .unwrap()
            .unwrap();
        assert_eq!(tests.column, Column::Backlog);
        assert!(!tests.auto_complete, "review = true");
        assert_eq!(tests.max_retries, 2);
        assert_eq!(
            tests.placement,
            Placement::Worktree {
                branch: "board/{card}".into(),
                base: Some("main".into())
            }
        );

        // The `enqueue` key was resolved to the sibling card's real id.
        let rules = store.rules_for_card(&review.id, None).unwrap();
        let enqueue = rules
            .iter()
            .find_map(|r| match &r.action {
                Action::Enqueue { cards } => Some(cards.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(enqueue, vec![tests.id]);

        let timed = rules
            .iter()
            .find(|r| matches!(r.trigger, Trigger::WaitingFor { .. }))
            .unwrap();
        assert_eq!(timed.trigger, Trigger::WaitingFor { seconds: 900 });
        assert_eq!(timed.max_fires, 1);
    }

    #[test]
    fn re_syncing_updates_the_definition_and_leaves_live_state_alone() {
        let store = Store::open_in_memory().unwrap();
        let dir = repo_dir(Some(SAMPLE));
        let report = sync_repo(&store, dir.path(), "codex").unwrap();
        let repo_id = report.repo_id.clone();

        let card = store
            .find_card_by_key(&repo_id, "review-diff")
            .unwrap()
            .unwrap();
        store.set_lane(&card.id, Column::Running).unwrap();
        store.mark_dispatched(&card.id).unwrap();

        let changed = SAMPLE.replace("Review the diff", "Review the diff carefully");
        std::fs::write(dir.path().join(OVERLAY_FILE), changed).unwrap();
        let second = sync_repo(&store, dir.path(), "codex").unwrap();

        assert_eq!(second.created, 0);
        assert_eq!(second.updated, 2, "re-sync must not duplicate cards");

        let after = store.get_card(&card.id).unwrap().unwrap();
        assert_eq!(after.title, "Review the diff carefully");
        assert_eq!(after.column, Column::Running, "live state is untouched");
        assert_eq!(after.attempts, 1);
        assert_eq!(store.list_cards().unwrap().len(), 2);
        // Rules are part of the definition, so they are replaced, not duplicated.
        assert_eq!(store.list_rules().unwrap().len(), 2);
    }

    #[test]
    fn a_repo_with_no_overlay_still_registers_itself() {
        let store = Store::open_in_memory().unwrap();
        let dir = repo_dir(None);
        let report = sync_repo(&store, dir.path(), "claude").unwrap();
        assert_eq!(report.created, 0);
        let repo = store.get_repo(&report.repo_id).unwrap().unwrap();
        assert_eq!(repo.name, dir.path().file_name().unwrap().to_string_lossy());
    }

    #[test]
    fn a_rule_with_no_action_is_rejected_with_a_useful_message() {
        let store = Store::open_in_memory().unwrap();
        let dir = repo_dir(Some(
            "[[card]]\nkey = \"a\"\n\n  [[card.rules]]\n  on = \"done\"\n",
        ));
        let err = sync_repo(&store, dir.path(), "claude")
            .unwrap_err()
            .to_string();
        assert!(err.contains("declares no action"), "got: {err}");
    }

    #[test]
    fn a_rule_with_two_actions_is_rejected() {
        let store = Store::open_in_memory().unwrap();
        let dir = repo_dir(Some(
            "[[card]]\nkey = \"a\"\n\n  [[card.rules]]\n  on = \"done\"\n  cancel = true\n  notify = \"x\"\n",
        ));
        let err = sync_repo(&store, dir.path(), "claude")
            .unwrap_err()
            .to_string();
        assert!(err.contains("one rule per action"), "got: {err}");
    }

    #[test]
    fn a_waiting_rule_without_a_duration_is_rejected() {
        let store = Store::open_in_memory().unwrap();
        let dir = repo_dir(Some(
            "[[card]]\nkey = \"a\"\n\n  [[card.rules]]\n  on = \"waiting\"\n  cancel = true\n",
        ));
        let err = sync_repo(&store, dir.path(), "claude")
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs an `after` duration"), "got: {err}");
    }

    #[test]
    fn a_typo_in_the_overlay_is_reported_rather_than_ignored() {
        let store = Store::open_in_memory().unwrap();
        let dir = repo_dir(Some("[[card]]\nkey = \"a\"\nprommpt = \"oops\"\n"));
        assert!(sync_repo(&store, dir.path(), "claude").is_err());
    }
}
