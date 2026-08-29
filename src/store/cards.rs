//! Card persistence and the lane transition that the whole engine funnels through.

use anyhow::{bail, Result};
use rusqlite::{params, Row};

use super::{from_json, new_id, now, to_json, Store};
use crate::model::{Binding, Card, Column, Placement};

const SELECT: &str =
    "SELECT id, key, title, prompt, repo_id, session, tags, agent_kind, model, extra_args, \
     placement, lane, binding, priority, auto_complete, auto_answer, max_retries, attempts, \
     created_at, updated_at, status_since, dispatched_at, last_error, prompt_sent FROM cards";

fn row_to_card(r: &Row<'_>) -> rusqlite::Result<Card> {
    Ok(Card {
        id: r.get("id")?,
        key: r.get("key")?,
        title: r.get("title")?,
        prompt: r.get("prompt")?,
        repo_id: r.get("repo_id")?,
        session: r.get("session")?,
        tags: from_json(&r.get::<_, String>("tags")?)?,
        agent_kind: r.get("agent_kind")?,
        model: r.get("model")?,
        extra_args: from_json(&r.get::<_, String>("extra_args")?)?,
        placement: from_json(&r.get::<_, String>("placement")?)?,
        column: r
            .get::<_, String>("lane")?
            .parse()
            .map_err(|e: anyhow::Error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    e.to_string().into(),
                )
            })?,
        binding: from_json(&r.get::<_, String>("binding")?)?,
        priority: r.get("priority")?,
        auto_complete: r.get("auto_complete")?,
        auto_answer: r.get("auto_answer")?,
        max_retries: r.get::<_, i64>("max_retries")? as u32,
        attempts: r.get::<_, i64>("attempts")? as u32,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        status_since: r.get("status_since")?,
        dispatched_at: r.get("dispatched_at")?,
        last_error: r.get("last_error")?,
        prompt_sent: r.get("prompt_sent")?,
    })
}

/// Everything needed to create a card. Defaults come from the repo and config.
#[derive(Debug, Clone, PartialEq)]
pub struct NewCard {
    pub key: Option<String>,
    pub title: String,
    pub prompt: String,
    pub repo_id: Option<String>,
    /// Which herdr session should run this. Defaults to the one we are in.
    pub session: Option<String>,
    pub tags: Vec<String>,
    pub agent_kind: String,
    pub model: Option<String>,
    pub extra_args: Vec<String>,
    pub placement: Placement,
    pub column: Column,
    pub priority: i64,
    pub auto_complete: bool,
    pub auto_answer: bool,
    pub max_retries: u32,
}

impl NewCard {
    pub fn new(title: impl Into<String>, agent_kind: impl Into<String>) -> Self {
        Self {
            key: None,
            title: title.into(),
            prompt: String::new(),
            repo_id: None,
            // Claimed for the session we are running in, so another session's
            // event hook will not start it.
            session: crate::session::current_name(),
            tags: vec![],
            agent_kind: agent_kind.into(),
            model: None,
            extra_args: vec![],
            placement: Placement::default(),
            column: Column::Backlog,
            priority: 0,
            // An agentic card is finished when its agent's turn ends. Set this to
            // false to park the card in `waiting` for a human instead.
            auto_complete: true,
            auto_answer: false,
            max_retries: 0,
        }
    }
}

impl Store {
    pub fn create_card(&self, new: &NewCard) -> Result<Card> {
        let ts = now();
        let card = Card {
            id: new_id(),
            key: new.key.clone(),
            title: new.title.clone(),
            prompt: new.prompt.clone(),
            repo_id: new.repo_id.clone(),
            session: new.session.clone(),
            tags: new.tags.clone(),
            agent_kind: new.agent_kind.clone(),
            model: new.model.clone(),
            extra_args: new.extra_args.clone(),
            placement: new.placement.clone(),
            column: new.column,
            binding: Binding::default(),
            priority: new.priority,
            auto_complete: new.auto_complete,
            auto_answer: new.auto_answer,
            max_retries: new.max_retries,
            attempts: 0,
            created_at: ts,
            updated_at: ts,
            status_since: ts,
            dispatched_at: None,
            last_error: None,
            prompt_sent: false,
        };
        self.insert_card(&card)?;
        Ok(card)
    }

    fn insert_card(&self, c: &Card) -> Result<()> {
        self.conn().execute(
            "INSERT INTO cards (id, key, title, prompt, repo_id, session, tags, agent_kind, model,
                extra_args, placement, lane, binding, priority, auto_complete, auto_answer,
                max_retries, attempts, created_at, updated_at, status_since, dispatched_at,
                last_error, prompt_sent)
             VALUES (?1,?2,?3,?4,?5,?24,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
            params![
                c.id,
                c.key,
                c.title,
                c.prompt,
                c.repo_id,
                to_json(&c.tags)?,
                c.agent_kind,
                c.model,
                to_json(&c.extra_args)?,
                to_json(&c.placement)?,
                c.column.as_str(),
                to_json(&c.binding)?,
                c.priority,
                c.auto_complete,
                c.auto_answer,
                c.max_retries as i64,
                c.attempts as i64,
                c.created_at,
                c.updated_at,
                c.status_since,
                c.dispatched_at,
                c.last_error,
                c.prompt_sent,
                c.session,
            ],
        )?;
        Ok(())
    }

    /// Full overwrite of an existing card. Bumps `updated_at`; does not touch
    /// `status_since`, which only [`Store::set_lane`] may move.
    pub fn update_card(&self, c: &Card) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE cards SET key=?2, title=?3, prompt=?4, repo_id=?5, tags=?6, agent_kind=?7,
                session=?23,
                model=?8, extra_args=?9, placement=?10, lane=?11, binding=?12, priority=?13,
                auto_complete=?14, auto_answer=?15, max_retries=?16, attempts=?17,
                updated_at=?18, status_since=?19, dispatched_at=?20, last_error=?21,
                prompt_sent=?22
             WHERE id=?1",
            params![
                c.id,
                c.key,
                c.title,
                c.prompt,
                c.repo_id,
                to_json(&c.tags)?,
                c.agent_kind,
                c.model,
                to_json(&c.extra_args)?,
                to_json(&c.placement)?,
                c.column.as_str(),
                to_json(&c.binding)?,
                c.priority,
                c.auto_complete,
                c.auto_answer,
                c.max_retries as i64,
                c.attempts as i64,
                now(),
                c.status_since,
                c.dispatched_at,
                c.last_error,
                c.prompt_sent,
                c.session,
            ],
        )?;
        if n == 0 {
            bail!("card {} does not exist", c.id);
        }
        Ok(())
    }

    pub fn get_card(&self, id: &str) -> Result<Option<Card>> {
        let mut stmt = self.conn().prepare(&format!("{SELECT} WHERE id = ?1"))?;
        let mut rows = stmt.query_map([id], row_to_card)?;
        Ok(rows.next().transpose()?)
    }

    /// Resolve a card the way a human refers to one: full id, id prefix, `repo:key`,
    /// bare `key`, or exact title.
    pub fn resolve_card(&self, needle: &str) -> Result<Option<Card>> {
        if let Some(c) = self.get_card(needle)? {
            return Ok(Some(c));
        }
        let queries: Vec<(String, Vec<String>)> = vec![
            (
                format!("{SELECT} WHERE id LIKE ?1 ORDER BY id LIMIT 2"),
                vec![format!("{needle}%")],
            ),
            (
                format!("{SELECT} WHERE key = ?1 ORDER BY created_at LIMIT 2"),
                vec![needle.to_string()],
            ),
            (
                format!("{SELECT} WHERE title = ?1 ORDER BY created_at LIMIT 2"),
                vec![needle.to_string()],
            ),
            (
                format!("{SELECT} WHERE lower(title) = lower(?1) ORDER BY created_at LIMIT 2"),
                vec![needle.to_string()],
            ),
        ];
        for (sql, args) in queries {
            let mut stmt = self.conn().prepare(&sql)?;
            let found = stmt
                .query_map(rusqlite::params_from_iter(args), row_to_card)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            match found.len() {
                0 => continue,
                1 => return Ok(Some(found.into_iter().next().unwrap())),
                _ => bail!("{needle:?} is ambiguous; use the full card id"),
            }
        }
        Ok(None)
    }

    /// Look up an overlay card by its `(repo, key)` natural identity.
    pub fn find_card_by_key(&self, repo_id: &str, key: &str) -> Result<Option<Card>> {
        let mut stmt = self
            .conn()
            .prepare(&format!("{SELECT} WHERE repo_id = ?1 AND key = ?2"))?;
        let mut rows = stmt.query_map([repo_id, key], row_to_card)?;
        Ok(rows.next().transpose()?)
    }

    /// Every card, in board order.
    ///
    /// The `rowid` tiebreak matters. `created_at` is whole seconds, so cards made
    /// in the same second tie, and SQLite then returns them in whatever order it
    /// likes — a dispatch order that differed between machines. `id` is not a
    /// substitute: a ULID only sorts by time across milliseconds, and two
    /// generated inside one are ordered by their random tail. `rowid` is
    /// insertion order, which is what "created first" actually means here.
    pub fn list_cards(&self) -> Result<Vec<Card>> {
        let mut stmt = self.conn().prepare(&format!(
            "{SELECT} ORDER BY priority DESC, created_at, rowid"
        ))?;
        let rows = stmt
            .query_map([], row_to_card)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn cards_in(&self, column: Column) -> Result<Vec<Card>> {
        let mut stmt = self.conn().prepare(&format!(
            "{SELECT} WHERE lane = ?1 ORDER BY priority DESC, created_at, rowid"
        ))?;
        let rows = stmt
            .query_map([column.as_str()], row_to_card)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Cards currently holding a herdr pane, used for concurrency accounting.
    pub fn live_cards(&self) -> Result<Vec<Card>> {
        let mut stmt = self.conn().prepare(&format!(
            "{SELECT} WHERE lane IN ('running','waiting','blocked') ORDER BY status_since, rowid"
        ))?;
        let rows = stmt
            .query_map([], row_to_card)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The card bound to a herdr pane, if any. The engine uses this to turn a
    /// pane-scoped event into a card.
    pub fn card_for_pane(&self, pane_id: &str) -> Result<Option<Card>> {
        let mut stmt = self.conn().prepare(&format!(
            "{SELECT} WHERE json_extract(binding, '$.pane_id') = ?1 \
             AND lane IN ('running','waiting','blocked') ORDER BY status_since DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query_map([pane_id], row_to_card)?;
        Ok(rows.next().transpose()?)
    }

    pub fn card_for_agent(&self, agent_name: &str) -> Result<Option<Card>> {
        let mut stmt = self.conn().prepare(&format!(
            "{SELECT} WHERE json_extract(binding, '$.agent_name') = ?1 \
             AND lane IN ('running','waiting','blocked') ORDER BY status_since DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query_map([agent_name], row_to_card)?;
        Ok(rows.next().transpose()?)
    }

    /// Move a card to a new lane.
    ///
    /// `status_since` only advances on a real lane change, because the timed rules
    /// ("waiting for more than 15 minutes") measure from it. A redundant write must
    /// not reset that clock.
    pub fn set_lane(&self, card_id: &str, lane: Column) -> Result<bool> {
        let changed = self.conn().execute(
            "UPDATE cards SET lane = ?2, status_since = ?3, updated_at = ?3
             WHERE id = ?1 AND lane <> ?2",
            params![card_id, lane.as_str(), now()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_binding(&self, card_id: &str, binding: &Binding) -> Result<()> {
        self.conn().execute(
            "UPDATE cards SET binding = ?2, updated_at = ?3 WHERE id = ?1",
            params![card_id, to_json(binding)?, now()],
        )?;
        Ok(())
    }

    pub fn clear_binding(&self, card_id: &str) -> Result<()> {
        self.set_binding(card_id, &Binding::default())
    }

    pub fn set_error(&self, card_id: &str, error: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE cards SET last_error = ?2, updated_at = ?3 WHERE id = ?1",
            params![card_id, error, now()],
        )?;
        Ok(())
    }

    pub fn set_prompt_sent(&self, card_id: &str, sent: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE cards SET prompt_sent = ?2, updated_at = ?3 WHERE id = ?1",
            params![card_id, sent, now()],
        )?;
        Ok(())
    }

    /// Record a dispatch attempt.
    pub fn mark_dispatched(&self, card_id: &str) -> Result<u32> {
        self.conn().execute(
            "UPDATE cards SET attempts = attempts + 1, dispatched_at = ?2, updated_at = ?2
             WHERE id = ?1",
            params![card_id, now()],
        )?;
        let attempts: i64 =
            self.conn()
                .query_row("SELECT attempts FROM cards WHERE id = ?1", [card_id], |r| {
                    r.get(0)
                })?;
        Ok(attempts as u32)
    }

    /// Move a card up or down within its lane.
    ///
    /// Lanes are ordered by `priority DESC, created_at`, which means priority is
    /// only ever compared between neighbours. So rather than invent a scale, the
    /// whole lane is renumbered from the reordered list — a handful of rows, and
    /// the result is always exactly the order shown.
    pub fn reorder_in_lane(&mut self, card_id: &str, delta: i64) -> Result<bool> {
        let Some(card) = self.get_card(card_id)? else {
            return Ok(false);
        };
        let mut lane = self.cards_in(card.column)?;
        let Some(from) = lane.iter().position(|c| c.id == card_id) else {
            return Ok(false);
        };
        let to = (from as i64 + delta).clamp(0, lane.len() as i64 - 1) as usize;
        if to == from {
            return Ok(false);
        }
        let moved = lane.remove(from);
        lane.insert(to, moved);

        let top = lane.len() as i64;
        self.transaction(|tx| {
            for (i, c) in lane.iter().enumerate() {
                tx.execute(
                    "UPDATE cards SET priority = ?2, updated_at = ?3 WHERE id = ?1",
                    params![c.id, top - i as i64, now()],
                )?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// Clear the dispatch count, so a card stopped by the budget can run again.
    /// Deliberate: only an explicit retry does this.
    pub fn reset_attempts(&self, card_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE cards SET attempts = 0, updated_at = ?2 WHERE id = ?1",
            params![card_id, now()],
        )?;
        Ok(())
    }

    pub fn delete_card(&self, id: &str) -> Result<bool> {
        let n = self
            .conn()
            .execute("DELETE FROM cards WHERE id = ?1", [id])?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(store: &Store, title: &str) -> Card {
        store.create_card(&NewCard::new(title, "claude")).unwrap()
    }

    #[test]
    fn set_lane_only_moves_the_clock_on_a_real_change() {
        let store = Store::open_in_memory().unwrap();
        let card = seed(&store, "a");
        assert!(store.set_lane(&card.id, Column::Running).unwrap());
        let first = store.get_card(&card.id).unwrap().unwrap();

        // A redundant write must not reset status_since: the waiting-for rules read it.
        assert!(!store.set_lane(&card.id, Column::Running).unwrap());
        let second = store.get_card(&card.id).unwrap().unwrap();
        assert_eq!(first.status_since, second.status_since);
        assert_eq!(second.column, Column::Running);
    }

    #[test]
    fn card_for_pane_finds_only_live_bindings() {
        let store = Store::open_in_memory().unwrap();
        let card = seed(&store, "a");
        store
            .set_binding(
                &card.id,
                &Binding {
                    pane_id: Some("w1:p3".into()),
                    agent_name: Some("a-abc123".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        // Still in backlog: not live, so a pane event must not resolve to it.
        assert!(store.card_for_pane("w1:p3").unwrap().is_none());

        store.set_lane(&card.id, Column::Running).unwrap();
        assert_eq!(store.card_for_pane("w1:p3").unwrap().unwrap().id, card.id);
        assert_eq!(
            store.card_for_agent("a-abc123").unwrap().unwrap().id,
            card.id
        );

        store.set_lane(&card.id, Column::Done).unwrap();
        assert!(store.card_for_pane("w1:p3").unwrap().is_none());
    }

    #[test]
    fn live_cards_covers_exactly_the_live_columns() {
        let store = Store::open_in_memory().unwrap();
        for column in Column::ALL {
            let c = seed(&store, column.as_str());
            store.set_lane(&c.id, column).unwrap();
        }
        let live: Vec<_> = store
            .live_cards()
            .unwrap()
            .into_iter()
            .map(|c| c.column)
            .collect();
        assert_eq!(live.len(), 3);
        for column in live {
            assert!(column.is_live());
        }
    }

    #[test]
    fn resolve_card_accepts_id_prefix_and_title_but_rejects_ambiguity() {
        let store = Store::open_in_memory().unwrap();
        let card = seed(&store, "Review the diff");
        assert_eq!(
            store.resolve_card(&card.id[..8]).unwrap().unwrap().id,
            card.id
        );
        assert_eq!(
            store.resolve_card("Review the diff").unwrap().unwrap().id,
            card.id
        );
        seed(&store, "Review the diff");
        assert!(store.resolve_card("Review the diff").is_err());
    }

    #[test]
    fn a_card_round_trips_every_field() {
        let store = Store::open_in_memory().unwrap();
        let mut card = store
            .create_card(&NewCard {
                prompt: "do the thing".into(),
                tags: vec!["urgent".into()],
                model: Some("opus".into()),
                extra_args: vec!["--yolo".into()],
                placement: Placement::Worktree {
                    branch: "feat/x".into(),
                    base: Some("main".into()),
                },
                auto_answer: true,
                max_retries: 3,
                ..NewCard::new("full", "codex")
            })
            .unwrap();
        card.binding.pane_id = Some("w2:p1".into());
        card.last_error = Some("boom".into());
        store.update_card(&card).unwrap();

        let back = store.get_card(&card.id).unwrap().unwrap();
        assert_eq!(back.placement, card.placement);
        assert_eq!(back.tags, card.tags);
        assert_eq!(back.extra_args, card.extra_args);
        assert_eq!(back.binding.pane_id.as_deref(), Some("w2:p1"));
        assert_eq!(back.last_error.as_deref(), Some("boom"));
        assert!(back.auto_answer);
    }

    /// Cards created in the same second must still come back in the order they
    /// were made. This broke on CI while passing locally, because the tiebreak
    /// was the ULID's random tail rather than insertion order.
    #[test]
    fn cards_made_in_the_same_second_keep_their_order() {
        for _ in 0..20 {
            let store = Store::open_in_memory().unwrap();
            let made: Vec<String> = (0..8)
                .map(|i| seed(&store, &format!("card {i}")).title)
                .collect();
            let listed: Vec<String> = store
                .list_cards()
                .unwrap()
                .into_iter()
                .map(|c| c.title)
                .collect();
            assert_eq!(listed, made);
        }
    }

    #[test]
    fn reordering_moves_a_card_within_its_lane_and_nowhere_else() {
        let mut store = Store::open_in_memory().unwrap();
        let ids: Vec<String> = ["a", "b", "c"].iter().map(|t| seed(&store, t).id).collect();
        let order = |s: &Store| {
            s.cards_in(Column::Backlog)
                .unwrap()
                .into_iter()
                .map(|c| c.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&store), ["a", "b", "c"]);

        assert!(store.reorder_in_lane(&ids[2], -1).unwrap());
        assert_eq!(order(&store), ["a", "c", "b"]);

        assert!(store.reorder_in_lane(&ids[2], -1).unwrap());
        assert_eq!(order(&store), ["c", "a", "b"]);

        // Already at the top: nothing to do, and it says so.
        assert!(!store.reorder_in_lane(&ids[2], -1).unwrap());
        assert_eq!(order(&store), ["c", "a", "b"]);
    }

    #[test]
    fn reordering_leaves_other_lanes_untouched() {
        let mut store = Store::open_in_memory().unwrap();
        let a = seed(&store, "a");
        let b = seed(&store, "b");
        let elsewhere = seed(&store, "z");
        store.set_lane(&elsewhere.id, Column::Ready).unwrap();
        let before = store.get_card(&elsewhere.id).unwrap().unwrap().priority;

        store.reorder_in_lane(&b.id, -1).unwrap();

        assert_eq!(
            store.get_card(&elsewhere.id).unwrap().unwrap().priority,
            before
        );
        assert_eq!(store.cards_in(Column::Backlog).unwrap()[0].id, b.id);
        let _ = a;
    }

    #[test]
    fn reordering_a_card_that_is_gone_is_a_no_op() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(!store.reorder_in_lane("nope", -1).unwrap());
    }

    #[test]
    fn updating_a_missing_card_is_an_error_not_a_silent_no_op() {
        let store = Store::open_in_memory().unwrap();
        let mut card = seed(&store, "a");
        store.delete_card(&card.id).unwrap();
        card.title = "b".into();
        assert!(store.update_card(&card).is_err());
    }
}
