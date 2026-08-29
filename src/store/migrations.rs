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
