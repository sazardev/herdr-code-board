//! Dispatch history. One row per attempt, so a card's past is auditable.

use anyhow::Result;
use rusqlite::{params, Row};

use super::{new_id, now, Store};
use crate::model::Run;

const SELECT: &str = "SELECT id, card_id, attempt, started_at, ended_at, outcome, detail FROM runs";

fn row_to_run(r: &Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get("id")?,
        card_id: r.get("card_id")?,
        attempt: r.get::<_, i64>("attempt")? as u32,
        started_at: r.get("started_at")?,
        ended_at: r.get("ended_at")?,
        outcome: r.get("outcome")?,
        detail: r.get("detail")?,
    })
}

impl Store {
    pub fn start_run(&self, card_id: &str, attempt: u32) -> Result<Run> {
        let run = Run {
            id: new_id(),
            card_id: card_id.to_string(),
            attempt,
            started_at: now(),
            ended_at: None,
            outcome: None,
            detail: None,
        };
        self.conn().execute(
            "INSERT INTO runs (id, card_id, attempt, started_at) VALUES (?1,?2,?3,?4)",
            params![run.id, run.card_id, run.attempt as i64, run.started_at],
        )?;
        Ok(run)
    }

    /// Close a run. A `None` detail leaves whatever notes the run accumulated;
    /// otherwise the audit trail written by [`Store::note_open_run`] — including
    /// what an approval dialog said before a rule answered it — would be erased
    /// the moment the run ends.
    pub fn finish_run(&self, run_id: &str, outcome: &str, detail: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE runs SET ended_at = ?2, outcome = ?3,
                detail = CASE
                    WHEN ?4 IS NULL THEN detail
                    WHEN detail IS NULL OR detail = '' THEN ?4
                    ELSE detail || char(10) || ?4
                END
             WHERE id = ?1",
            params![run_id, now(), outcome, detail],
        )?;
        Ok(())
    }

    /// Close the newest open run of a card. Used when an event, not the dispatcher,
    /// is what ends the attempt.
    pub fn finish_open_run(
        &self,
        card_id: &str,
        outcome: &str,
        detail: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE runs SET ended_at = ?2, outcome = ?3,
                detail = CASE
                    WHEN ?4 IS NULL THEN detail
                    WHEN detail IS NULL OR detail = '' THEN ?4
                    ELSE detail || char(10) || ?4
                END
             WHERE id = (SELECT id FROM runs WHERE card_id = ?1 AND ended_at IS NULL
                         ORDER BY started_at DESC LIMIT 1)",
            params![card_id, now(), outcome, detail],
        )?;
        Ok(())
    }

    /// Append a note to the newest open run without closing it. This is where the
    /// engine records what a blocked dialog said before it answered one.
    pub fn note_open_run(&self, card_id: &str, note: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE runs
             SET detail = CASE WHEN detail IS NULL OR detail = '' THEN ?2
                               ELSE detail || char(10) || ?2 END
             WHERE id = (SELECT id FROM runs WHERE card_id = ?1 AND ended_at IS NULL
                         ORDER BY started_at DESC LIMIT 1)",
            params![card_id, note],
        )?;
        Ok(())
    }

    pub fn runs_for_card(&self, card_id: &str, limit: u32) -> Result<Vec<Run>> {
        let mut stmt = self.conn().prepare(&format!(
            "{SELECT} WHERE card_id = ?1 ORDER BY started_at DESC LIMIT ?2"
        ))?;
        let rows = stmt
            .query_map(params![card_id, limit], row_to_run)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::cards::NewCard;

    #[test]
    fn finish_open_run_closes_only_the_newest_open_attempt() {
        let store = Store::open_in_memory().unwrap();
        let card = store.create_card(&NewCard::new("a", "claude")).unwrap();
        let first = store.start_run(&card.id, 1).unwrap();
        store.finish_run(&first.id, "failed", Some("boom")).unwrap();
        let second = store.start_run(&card.id, 2).unwrap();

        store.finish_open_run(&card.id, "done", None).unwrap();

        let runs = store.runs_for_card(&card.id, 10).unwrap();
        let second = runs.iter().find(|r| r.id == second.id).unwrap();
        let first = runs.iter().find(|r| r.id == first.id).unwrap();
        assert_eq!(second.outcome.as_deref(), Some("done"));
        assert_eq!(first.outcome.as_deref(), Some("failed"));
        assert_eq!(first.detail.as_deref(), Some("boom"));
    }

    /// Closing a run must not erase what happened during it. Found live: the
    /// dialog text recorded before an auto-answer was wiped by the `done`.
    #[test]
    fn finishing_a_run_preserves_the_notes_it_gathered() {
        let store = Store::open_in_memory().unwrap();
        let card = store.create_card(&NewCard::new("a", "claude")).unwrap();
        store.start_run(&card.id, 1).unwrap();
        store
            .note_open_run(&card.id, "dialog: trust this folder?")
            .unwrap();

        store.finish_open_run(&card.id, "done", None).unwrap();

        let run = &store.runs_for_card(&card.id, 1).unwrap()[0];
        assert_eq!(run.outcome.as_deref(), Some("done"));
        assert_eq!(
            run.detail.as_deref(),
            Some("dialog: trust this folder?"),
            "the audit trail must survive the run ending"
        );
    }

    #[test]
    fn a_closing_detail_is_appended_rather_than_replacing_the_notes() {
        let store = Store::open_in_memory().unwrap();
        let card = store.create_card(&NewCard::new("a", "claude")).unwrap();
        let run = store.start_run(&card.id, 1).unwrap();
        store.note_open_run(&card.id, "answered choice 2").unwrap();
        store
            .finish_run(&run.id, "failed", Some("pane vanished"))
            .unwrap();

        let detail = store.runs_for_card(&card.id, 1).unwrap()[0]
            .detail
            .clone()
            .unwrap();
        assert!(detail.contains("answered choice 2"));
        assert!(detail.contains("pane vanished"));
    }

    #[test]
    fn notes_accumulate_on_the_open_run() {
        let store = Store::open_in_memory().unwrap();
        let card = store.create_card(&NewCard::new("a", "claude")).unwrap();
        store.start_run(&card.id, 1).unwrap();
        store
            .note_open_run(&card.id, "dialog: allow edit?")
            .unwrap();
        store.note_open_run(&card.id, "answered choice 1").unwrap();
        let run = &store.runs_for_card(&card.id, 1).unwrap()[0];
        let detail = run.detail.clone().unwrap();
        assert!(detail.contains("dialog: allow edit?"));
        assert!(detail.contains("answered choice 1"));
    }
}
