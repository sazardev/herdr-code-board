//! Schema migrations, applied in order and tracked with `PRAGMA user_version`.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Each entry is one forward migration. Never edit a shipped entry; append.
const MIGRATIONS: &[&str] = &[
    // v1 — initial schema.
    r#"
    CREATE TABLE repos (
        id            TEXT PRIMARY KEY,
        name          TEXT NOT NULL,
        path          TEXT NOT NULL UNIQUE,
        tags          TEXT NOT NULL DEFAULT '[]',
        max_parallel  INTEGER NOT NULL DEFAULT 2,
        default_agent TEXT,
        default_model TEXT
    );

    CREATE TABLE cards (
        id            TEXT PRIMARY KEY,
        key           TEXT,
        title         TEXT NOT NULL,
        prompt        TEXT NOT NULL DEFAULT '',
        repo_id       TEXT REFERENCES repos(id) ON DELETE SET NULL,
        tags          TEXT NOT NULL DEFAULT '[]',
        agent_kind    TEXT NOT NULL,
        model         TEXT,
        extra_args    TEXT NOT NULL DEFAULT '[]',
        placement     TEXT NOT NULL,
        lane          TEXT NOT NULL,
        binding       TEXT NOT NULL DEFAULT '{}',
        priority      INTEGER NOT NULL DEFAULT 0,
        auto_complete INTEGER NOT NULL DEFAULT 0,
        auto_answer   INTEGER NOT NULL DEFAULT 0,
        max_retries   INTEGER NOT NULL DEFAULT 0,
        attempts      INTEGER NOT NULL DEFAULT 0,
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL,
        status_since  INTEGER NOT NULL,
        dispatched_at INTEGER,
        last_error    TEXT,
        prompt_sent   INTEGER NOT NULL DEFAULT 0
    );

    -- Repo overlay cards are identified by (repo, key); global cards have no key.
    CREATE UNIQUE INDEX cards_repo_key ON cards(repo_id, key) WHERE key IS NOT NULL;
    CREATE INDEX cards_lane ON cards(lane);
    CREATE INDEX cards_repo ON cards(repo_id);

    CREATE TABLE card_rules (
        id        TEXT PRIMARY KEY,
        card_id   TEXT REFERENCES cards(id) ON DELETE CASCADE,
        repo_id   TEXT REFERENCES repos(id) ON DELETE CASCADE,
        trigger   TEXT NOT NULL,
        action    TEXT NOT NULL,
        max_fires INTEGER NOT NULL DEFAULT 0,
        fired     INTEGER NOT NULL DEFAULT 0,
        enabled   INTEGER NOT NULL DEFAULT 1
    );

    CREATE INDEX card_rules_card ON card_rules(card_id);
    CREATE INDEX card_rules_repo ON card_rules(repo_id);

    CREATE TABLE runs (
        id         TEXT PRIMARY KEY,
        card_id    TEXT NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
        attempt    INTEGER NOT NULL,
        started_at INTEGER NOT NULL,
        ended_at   INTEGER,
        outcome    TEXT,
        detail     TEXT
    );

    CREATE INDEX runs_card ON runs(card_id, started_at DESC);

    CREATE TABLE event_log (
        seq     INTEGER PRIMARY KEY AUTOINCREMENT,
        at      INTEGER NOT NULL,
        kind    TEXT NOT NULL,
        card_id TEXT,
        detail  TEXT
    );

    CREATE TABLE kv (
        k TEXT PRIMARY KEY,
        v TEXT NOT NULL
    );
    "#,
    // v2 — which herdr session a card belongs to. NULL means unclaimed: any
    // session may run it, which is what every single-session board looks like.
    r#"
    ALTER TABLE cards ADD COLUMN session TEXT;
    CREATE INDEX cards_session ON cards(session);
    "#,
];

pub fn apply(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let current = usize::try_from(current).context("negative user_version")?;
    if current > MIGRATIONS.len() {
        anyhow::bail!(
            "database is at schema v{current} but this build only knows v{}; \
             upgrade herdr-code-board",
            MIGRATIONS.len()
        );
    }
    for (idx, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let version = idx + 1;
        conn.execute_batch(sql)
            .with_context(|| format!("applying migration v{version}"))?;
        conn.pragma_update(None, "user_version", version as i64)?;
    }
    Ok(())
}

/// The schema version this build produces.
pub fn latest_version() -> usize {
    MIGRATIONS.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upgrading an existing board must keep every card. A user who installs a
    /// new version has a database full of work in progress.
    #[test]
    fn upgrading_a_v1_database_keeps_its_cards() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1i64).unwrap();
        conn.execute(
            "INSERT INTO cards (id, title, prompt, agent_kind, placement, lane,
                created_at, updated_at, status_since)
             VALUES ('OLD', 'from before', 'p', 'claude', '{\"kind\":\"reuse\"}',
                     'running', 1, 1, 1)",
            [],
        )
        .unwrap();

        apply(&conn).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version as usize, latest_version());

        let (title, session): (String, Option<String>) = conn
            .query_row(
                "SELECT title, session FROM cards WHERE id = 'OLD'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "from before");
        assert_eq!(
            session, None,
            "a card from before sessions existed is unclaimed, so any session may run it"
        );
    }

    #[test]
    fn applying_from_scratch_reaches_the_same_place_as_upgrading() {
        let fresh = Connection::open_in_memory().unwrap();
        apply(&fresh).unwrap();

        let stepwise = Connection::open_in_memory().unwrap();
        stepwise.execute_batch(MIGRATIONS[0]).unwrap();
        stepwise.pragma_update(None, "user_version", 1i64).unwrap();
        apply(&stepwise).unwrap();

        let columns = |c: &Connection| -> Vec<String> {
            let mut stmt = c
                .prepare("SELECT name FROM pragma_table_info('cards')")
                .unwrap();
            let mut out: Vec<String> = stmt
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            out.sort();
            out
        };
        assert_eq!(columns(&fresh), columns(&stepwise));
    }
}
