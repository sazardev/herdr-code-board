//! What the board shows inside herdr itself.
//!
//! Herdr's Agent and Spaces sidebars render `$name` tokens that plugins publish
//! with `pane.report_metadata` and `workspace.report_metadata`. Publishing those
//! is how a card stops being something you only see on the board and becomes
//! something you see wherever you already are.
//!
//! The hard rule, inherited from herdr-agent-quota: **a metadata write can
//! repaint the pane a human is watching**. This plugin reacts to every agent
//! state change on the machine, so it must publish only when a value actually
//! changed. [`Publisher`] keeps the last published set and compares.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use crate::herdr::{HerdrApi, Tokens};
use crate::model::{Action, Card, Column, Rule};
use crate::store::Store;

/// The `--source` every write from this plugin carries, so herdr can attribute
/// and replace our tokens without touching anyone else's.
pub const SOURCE: &str = "herdr-code-board";

/// kv keys holding the last published state.
const PANE_STATE: &str = "published.panes";
const WORKSPACE_STATE: &str = "published.workspaces";

/// A one-character hint at what a card is doing, for a sidebar that has no room
/// for a word.
pub fn glyph(column: Column) -> &'static str {
    match column {
        Column::Backlog => "·",
        Column::Ready => "◷",
        Column::Running => "▶",
        Column::Waiting => "◍",
        Column::Blocked => "⚠",
        Column::Review => "◆",
        Column::Done => "✓",
        Column::Failed => "✗",
        Column::Cancelled => "–",
    }
}

/// The tokens a card publishes onto the pane it owns.
pub fn pane_tokens(card: &Card, rules: &[Rule], next: &[String], queued: usize) -> Tokens {
    let mut out: Tokens = vec![(
        "board_card".into(),
        format!("{} {}", glyph(card.column), card.title),
    )];

    // What this card will start when it finishes. The single most useful thing
    // to know about a running agent that you did not queue by hand.
    let chained = rules.iter().any(|r| {
        matches!(r.action, Action::Enqueue { .. })
            && r.trigger.watched_column() == Some(Column::Done)
    });
    out.push((
        "board_next".into(),
        if chained && !next.is_empty() {
            format!("→ {}", next.join(", "))
        } else {
            String::new()
        },
    ));

    let mut meta = Vec::new();
    if card.attempts > 1 {
        meta.push(format!("attempt {}", card.attempts));
    }
    if queued > 0 {
        meta.push(format!("{queued} queued"));
    }
    out.push(("board_meta".into(), meta.join(" · ")));
    out
}

/// The token a workspace publishes: how much of the board is live in it.
pub fn workspace_token(running: usize, queued: usize) -> String {
    match (running, queued) {
        (0, 0) => String::new(),
        (r, 0) => format!("▶ {r}"),
        (0, q) => format!("◷ {q}"),
        (r, q) => format!("▶ {r} · ◷ {q}"),
    }
}

/// Publishes board state into herdr's sidebars, writing only what changed.
pub struct Publisher;

impl Publisher {
    /// Recompute every token and push the differences.
    ///
    /// Returns how many writes were actually sent, which the tests assert on:
    /// a no-op sweep must send zero.
    pub fn publish(store: &Store, herdr: &dyn HerdrApi) -> Result<usize> {
        let cards = store.list_cards()?;
        let queued_by_repo = count_by_repo(&cards, Column::Ready);

        // --- panes ---------------------------------------------------------
        let mut wanted: BTreeMap<String, Tokens> = BTreeMap::new();
        for card in cards.iter().filter(|c| c.column.is_live()) {
            let Some(pane) = card.binding.pane_id.clone() else {
                continue;
            };
            let rules = store.rules_for_card(&card.id, card.repo_id.as_deref())?;
            let next = followers(store, &rules)?;
            let queued = card
                .repo_id
                .as_ref()
                .and_then(|r| queued_by_repo.get(r).copied())
                .unwrap_or(0);
            wanted.insert(pane, pane_tokens(card, &rules, &next, queued));
        }

        let mut writes = 0;
        writes += sync(store, PANE_STATE, &wanted, |id, tokens| {
            herdr.report_pane_tokens(id, SOURCE, tokens)
        })?;

        // --- workspaces ----------------------------------------------------
        let mut spaces: BTreeMap<String, Tokens> = BTreeMap::new();
        let mut running: BTreeMap<String, usize> = BTreeMap::new();
        let mut pending: BTreeMap<String, usize> = BTreeMap::new();
        for card in cards.iter().filter(|c| c.column.is_live()) {
            if let Some(ws) = &card.binding.workspace_id {
                *running.entry(ws.clone()).or_default() += 1;
            }
        }
        // Queued cards have no workspace yet, so attribute them to whichever
        // workspace their repo's live cards are already using.
        for card in cards.iter().filter(|c| c.column == Column::Ready) {
            let Some(repo) = &card.repo_id else { continue };
            let ws = cards.iter().find(|c| {
                c.column.is_live()
                    && c.repo_id.as_ref() == Some(repo)
                    && c.binding.workspace_id.is_some()
            });
            if let Some(ws) = ws.and_then(|c| c.binding.workspace_id.clone()) {
                *pending.entry(ws).or_default() += 1;
            }
        }
        for ws in running
            .keys()
            .chain(pending.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
        {
            let value = workspace_token(
                running.get(&ws).copied().unwrap_or(0),
                pending.get(&ws).copied().unwrap_or(0),
            );
            spaces.insert(ws, vec![("board_space".into(), value)]);
        }

        writes += sync(store, WORKSPACE_STATE, &spaces, |id, tokens| {
            herdr.report_workspace_tokens(id, SOURCE, tokens)
        })?;

        Ok(writes)
    }
}

/// Titles of the cards a set of `on done` rules would queue.
fn followers(store: &Store, rules: &[Rule]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for rule in rules {
        if rule.trigger.watched_column() != Some(Column::Done) {
            continue;
        }
        if let Action::Enqueue { cards } = &rule.action {
            for id in cards {
                if let Some(card) = store.get_card(id)? {
                    out.push(card.title);
                }
            }
        }
    }
    Ok(out)
}

fn count_by_repo(cards: &[Card], column: Column) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for card in cards.iter().filter(|c| c.column == column) {
        if let Some(repo) = &card.repo_id {
            *out.entry(repo.clone()).or_default() += 1;
        }
    }
    out
}

/// Write the difference between what is wanted and what was last published,
/// then remember the new state. Targets that dropped out get their tokens cleared.
fn sync(
    store: &Store,
    key: &str,
    wanted: &BTreeMap<String, Tokens>,
    mut write: impl FnMut(&str, &Tokens) -> Result<()>,
) -> Result<usize> {
    let previous: BTreeMap<String, Tokens> = store
        .kv_get(key)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    // A pane or workspace can disappear between publishes — closed, or living in
    // a herdr session that is no longer running. Neither a failed write nor a
    // failed clear may abort the sweep: the rest of the board still needs its
    // tokens, and a target we cannot reach is exactly one to forget.
    let mut writes = 0;
    let mut published: BTreeMap<String, Tokens> = BTreeMap::new();
    for (id, tokens) in wanted {
        if previous.get(id) == Some(tokens) {
            published.insert(id.clone(), tokens.clone());
            continue;
        }
        match write(id, tokens) {
            Ok(()) => {
                writes += 1;
                published.insert(id.clone(), tokens.clone());
            }
            Err(e) => {
                store.log_event("sidebar_write_failed", None, Some(&format!("{id}: {e}")))?;
            }
        }
    }

    // Anything we published to and no longer own must be cleared, or a finished
    // card's title lingers in the sidebar forever.
    for (id, tokens) in &previous {
        if wanted.contains_key(id) {
            continue;
        }
        let cleared: Tokens = tokens
            .iter()
            .map(|(k, _)| (k.clone(), String::new()))
            .collect();
        // Dropped either way: if the clear failed, the target is already gone.
        if write(id, &cleared).is_ok() {
            writes += 1;
        }
    }

    store.kv_set(key, &serde_json::to_string(&published)?)?;
    Ok(writes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::fake::FakeHerdr;
    use crate::model::{Binding, Repo, Trigger};
    use crate::store::cards::NewCard;

    fn board() -> (Store, Repo) {
        let store = Store::open_in_memory().unwrap();
        let repo = store
            .upsert_repo(&Repo {
                id: String::new(),
                name: "erp".into(),
                path: "/repo/erp".into(),
                tags: vec![],
                max_parallel: 4,
                default_agent: None,
                default_model: None,
            })
            .unwrap();
        (store, repo)
    }

    fn live(store: &Store, repo: &Repo, title: &str, pane: &str) -> Card {
        let card = store
            .create_card(&NewCard {
                repo_id: Some(repo.id.clone()),
                ..NewCard::new(title, "claude")
            })
            .unwrap();
        store
            .set_binding(
                &card.id,
                &Binding {
                    workspace_id: Some("w1".into()),
                    pane_id: Some(pane.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        store.set_lane(&card.id, Column::Running).unwrap();
        store.get_card(&card.id).unwrap().unwrap()
    }

    #[test]
    fn a_running_card_publishes_itself_onto_its_pane() {
        let (store, repo) = board();
        live(&store, &repo, "Review the diff", "w1:p2");
        let h = FakeHerdr::new();

        assert_eq!(Publisher::publish(&store, &h).unwrap(), 2);
        let call = &h.calls_matching("pane report-metadata")[0];
        assert!(call.contains("--source herdr-code-board"));
        assert!(call.contains("board_card=▶ Review the diff"), "got: {call}");
    }

    /// The rule that matters: the board reacts to every agent state change on the
    /// machine, and a redundant metadata write can repaint a pane a human is
    /// watching. A second publish with nothing changed must send nothing.
    #[test]
    fn publishing_twice_with_no_change_writes_nothing() {
        let (store, repo) = board();
        live(&store, &repo, "Review the diff", "w1:p2");
        let h = FakeHerdr::new();

        assert!(Publisher::publish(&store, &h).unwrap() > 0);
        let after_first = h.calls().len();
        assert_eq!(Publisher::publish(&store, &h).unwrap(), 0);
        assert_eq!(
            h.calls().len(),
            after_first,
            "no calls at all, not even no-ops"
        );
    }

    #[test]
    fn a_card_that_finishes_has_its_tokens_cleared() {
        let (store, repo) = board();
        let card = live(&store, &repo, "Review the diff", "w1:p2");
        let h = FakeHerdr::new();
        Publisher::publish(&store, &h).unwrap();

        store.set_lane(&card.id, Column::Done).unwrap();
        store.clear_binding(&card.id).unwrap();
        Publisher::publish(&store, &h).unwrap();

        let clears: Vec<String> = h
            .calls_matching("pane report-metadata w1:p2")
            .into_iter()
            .filter(|c| c.contains("-board_card"))
            .collect();
        assert_eq!(clears.len(), 1, "the title must not linger in the sidebar");
    }

    #[test]
    fn the_chained_follower_is_named_on_the_pane() {
        let (store, repo) = board();
        let first = live(&store, &repo, "Write it", "w1:p2");
        let second = store
            .create_card(&NewCard {
                repo_id: Some(repo.id.clone()),
                ..NewCard::new("Test it", "claude")
            })
            .unwrap();
        store
            .add_rule(
                Some(&first.id),
                None,
                &Trigger::Done,
                &Action::Enqueue {
                    cards: vec![second.id.clone()],
                },
                0,
            )
            .unwrap();

        let h = FakeHerdr::new();
        Publisher::publish(&store, &h).unwrap();
        let call = &h.calls_matching("pane report-metadata")[0];
        assert!(call.contains("board_next=→ Test it"), "got: {call}");
    }

    #[test]
    fn a_card_with_no_follower_clears_the_next_token_rather_than_inventing_one() {
        let (store, repo) = board();
        live(&store, &repo, "Alone", "w1:p2");
        let h = FakeHerdr::new();
        Publisher::publish(&store, &h).unwrap();
        let call = &h.calls_matching("pane report-metadata")[0];
        assert!(call.contains("-board_next"), "got: {call}");
    }

    #[test]
    fn the_workspace_shows_what_is_running_and_waiting_in_it() {
        let (store, repo) = board();
        live(&store, &repo, "One", "w1:p2");
        live(&store, &repo, "Two", "w1:p3");
        let queued = store
            .create_card(&NewCard {
                repo_id: Some(repo.id.clone()),
                ..NewCard::new("Three", "claude")
            })
            .unwrap();
        store.set_lane(&queued.id, Column::Ready).unwrap();

        let h = FakeHerdr::new();
        Publisher::publish(&store, &h).unwrap();
        let call = &h.calls_matching("workspace report-metadata")[0];
        assert!(call.contains("board_space=▶ 2 · ◷ 1"), "got: {call}");
    }

    /// A pane that vanished — closed, or in a herdr session that has stopped —
    /// must not take the whole sweep down with it.
    #[test]
    fn a_dead_target_is_forgotten_instead_of_failing_every_publish() {
        let (store, repo) = board();
        live(&store, &repo, "One", "w1:p2");
        let h = FakeHerdr::new();
        Publisher::publish(&store, &h).unwrap();

        // The card finishes, but the pane is already gone.
        let card = store.list_cards().unwrap()[0].clone();
        store.set_lane(&card.id, Column::Done).unwrap();
        store.clear_binding(&card.id).unwrap();
        h.fail_on("pane report-metadata", "pane_not_found");

        Publisher::publish(&store, &h).unwrap();

        // Forgotten: a later publish does not keep retrying it.
        let before = h.calls().len();
        Publisher::publish(&store, &h).unwrap();
        assert_eq!(h.calls().len(), before);
    }

    #[test]
    fn one_unreachable_pane_does_not_stop_the_others() {
        let (store, repo) = board();
        live(&store, &repo, "One", "w1:p2");
        live(&store, &repo, "Two", "w1:p3");
        let h = FakeHerdr::new();
        h.fail_on("pane report-metadata w1:p2", "pane_not_found");

        Publisher::publish(&store, &h).unwrap();
        assert_eq!(h.calls_matching("pane report-metadata w1:p3").len(), 1);

        // The one that failed is retried next time; the one that worked is not.
        let calls = h.calls().len();
        Publisher::publish(&store, &h).unwrap();
        let fresh: Vec<String> = h.calls().into_iter().skip(calls).collect();
        assert!(fresh.iter().any(|c| c.contains("w1:p2")));
        assert!(!fresh.iter().any(|c| c.contains("w1:p3")));
    }

    #[test]
    fn an_empty_board_publishes_nothing() {
        let store = Store::open_in_memory().unwrap();
        let h = FakeHerdr::new();
        assert_eq!(Publisher::publish(&store, &h).unwrap(), 0);
        assert!(h.calls().is_empty());
    }

    #[test]
    fn the_workspace_token_reads_naturally_at_every_count() {
        assert_eq!(workspace_token(0, 0), "");
        assert_eq!(workspace_token(2, 0), "▶ 2");
        assert_eq!(workspace_token(0, 3), "◷ 3");
        assert_eq!(workspace_token(2, 3), "▶ 2 · ◷ 3");
    }

    #[test]
    fn every_lane_has_a_glyph_and_they_are_distinct() {
        let all: BTreeSet<&str> = Column::ALL.iter().map(|c| glyph(*c)).collect();
        assert_eq!(all.len(), Column::ALL.len());
    }
}
