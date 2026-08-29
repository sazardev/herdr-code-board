//! Automation rules: what happens when a card reaches a state, or sits in one too long.

use anyhow::Result;
use rusqlite::{params, Row};

use super::{from_json, new_id, to_json, Store};
use crate::model::{Action, Rule, Trigger};

const SELECT: &str =
    "SELECT id, card_id, repo_id, trigger, action, max_fires, fired, enabled FROM card_rules";

fn row_to_rule(r: &Row<'_>) -> rusqlite::Result<Rule> {
    Ok(Rule {
        id: r.get("id")?,
        card_id: r.get("card_id")?,
        repo_id: r.get("repo_id")?,
        trigger: from_json(&r.get::<_, String>("trigger")?)?,
        action: from_json(&r.get::<_, String>("action")?)?,
        max_fires: r.get::<_, i64>("max_fires")? as u32,
        fired: r.get::<_, i64>("fired")? as u32,
        enabled: r.get("enabled")?,
    })
}

impl Store {
    pub fn add_rule(
        &self,
        card_id: Option<&str>,
        repo_id: Option<&str>,
        trigger: &Trigger,
        action: &Action,
        max_fires: u32,
    ) -> Result<Rule> {
        let rule = Rule {
            id: new_id(),
            card_id: card_id.map(str::to_string),
            repo_id: repo_id.map(str::to_string),
            trigger: trigger.clone(),
            action: action.clone(),
            max_fires,
            fired: 0,
            enabled: true,
        };
        self.conn().execute(
            "INSERT INTO card_rules (id, card_id, repo_id, trigger, action, max_fires, fired, enabled)
             VALUES (?1,?2,?3,?4,?5,?6,0,1)",
            params![
                rule.id,
                rule.card_id,
                rule.repo_id,
                to_json(&rule.trigger)?,
                to_json(&rule.action)?,
                rule.max_fires as i64,
            ],
        )?;
        Ok(rule)
    }

    /// Rules that apply to a card: its own, plus the ones inherited from its repo.
    pub fn rules_for_card(&self, card_id: &str, repo_id: Option<&str>) -> Result<Vec<Rule>> {
        let mut stmt = self.conn().prepare(&format!(
            "{SELECT} WHERE enabled = 1 AND (card_id = ?1 OR (card_id IS NULL AND repo_id IS NOT NULL AND repo_id = ?2))
             ORDER BY card_id IS NULL, id"
        ))?;
        let rows = stmt
            .query_map(params![card_id, repo_id], row_to_rule)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn list_rules(&self) -> Result<Vec<Rule>> {
        let mut stmt = self.conn().prepare(&format!("{SELECT} ORDER BY id"))?;
        let rows = stmt
            .query_map([], row_to_rule)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Record that a rule fired. Returns false when it had already hit `max_fires`,
    /// which is how the engine avoids re-firing the same rule in a loop.
    pub fn try_consume_rule(&self, rule_id: &str) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE card_rules SET fired = fired + 1
             WHERE id = ?1 AND enabled = 1 AND (max_fires = 0 OR fired < max_fires)",
            [rule_id],
        )?;
        Ok(n > 0)
    }

    /// Reset fire counters for a card's rules; called when a card is re-dispatched.
    pub fn reset_rule_fires(&self, card_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE card_rules SET fired = 0 WHERE card_id = ?1",
            [card_id],
        )?;
        Ok(())
    }

    pub fn set_rule_enabled(&self, rule_id: &str, enabled: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE card_rules SET enabled = ?2 WHERE id = ?1",
            params![rule_id, enabled],
        )?;
        Ok(())
    }

    pub fn delete_rule(&self, rule_id: &str) -> Result<bool> {
        let n = self
            .conn()
            .execute("DELETE FROM card_rules WHERE id = ?1", [rule_id])?;
        Ok(n > 0)
    }

    pub fn delete_rules_for_card(&self, card_id: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM card_rules WHERE card_id = ?1", [card_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Repo;
    use crate::store::cards::NewCard;

    #[test]
    fn a_card_sees_its_own_rules_and_its_repos() {
        let store = Store::open_in_memory().unwrap();
        let repo = store
            .upsert_repo(&Repo {
                id: String::new(),
                name: "erp".into(),
                path: "/tmp/erp".into(),
                tags: vec![],
                max_parallel: 2,
                default_agent: None,
                default_model: None,
            })
            .unwrap();
        let card = store
            .create_card(&NewCard {
                repo_id: Some(repo.id.clone()),
                ..NewCard::new("a", "claude")
            })
            .unwrap();
        let other = store.create_card(&NewCard::new("b", "claude")).unwrap();

        store
            .add_rule(Some(&card.id), None, &Trigger::Done, &Action::Cancel, 0)
            .unwrap();
        store
            .add_rule(
                None,
                Some(&repo.id),
                &Trigger::Failed,
                &Action::Notify {
                    title: "failed".into(),
                    body: None,
                },
                0,
            )
            .unwrap();
        store
            .add_rule(Some(&other.id), None, &Trigger::Done, &Action::Cancel, 0)
            .unwrap();

        let rules = store.rules_for_card(&card.id, Some(&repo.id)).unwrap();
        assert_eq!(rules.len(), 2, "own rule + inherited repo rule");
        // Card-scoped rules come first so they can pre-empt a repo default.
        assert!(rules[0].card_id.is_some());
    }

    #[test]
    fn max_fires_stops_a_rule_from_looping() {
        let store = Store::open_in_memory().unwrap();
        let card = store.create_card(&NewCard::new("a", "claude")).unwrap();
        let rule = store
            .add_rule(
                Some(&card.id),
                None,
                &Trigger::WaitingFor { seconds: 60 },
                &Action::Prompt {
                    text: "still there?".into(),
                },
                2,
            )
            .unwrap();
        assert!(store.try_consume_rule(&rule.id).unwrap());
        assert!(store.try_consume_rule(&rule.id).unwrap());
        assert!(
            !store.try_consume_rule(&rule.id).unwrap(),
            "the third fire must be refused"
        );

        store.reset_rule_fires(&card.id).unwrap();
        assert!(store.try_consume_rule(&rule.id).unwrap());
    }

    #[test]
    fn unlimited_rules_use_max_fires_zero() {
        let store = Store::open_in_memory().unwrap();
        let card = store.create_card(&NewCard::new("a", "claude")).unwrap();
        let rule = store
            .add_rule(Some(&card.id), None, &Trigger::Blocked, &Action::Cancel, 0)
            .unwrap();
        for _ in 0..10 {
            assert!(store.try_consume_rule(&rule.id).unwrap());
        }
    }

    #[test]
    fn disabled_rules_neither_list_for_a_card_nor_fire() {
        let store = Store::open_in_memory().unwrap();
        let card = store.create_card(&NewCard::new("a", "claude")).unwrap();
        let rule = store
            .add_rule(Some(&card.id), None, &Trigger::Done, &Action::Cancel, 0)
            .unwrap();
        store.set_rule_enabled(&rule.id, false).unwrap();
        assert!(store.rules_for_card(&card.id, None).unwrap().is_empty());
        assert!(!store.try_consume_rule(&rule.id).unwrap());
    }

    #[test]
    fn deleting_a_card_takes_its_rules_with_it() {
        let store = Store::open_in_memory().unwrap();
        let card = store.create_card(&NewCard::new("a", "claude")).unwrap();
        store
            .add_rule(Some(&card.id), None, &Trigger::Done, &Action::Cancel, 0)
            .unwrap();
        store.delete_card(&card.id).unwrap();
        assert!(store.list_rules().unwrap().is_empty());
    }
}
