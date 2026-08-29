//! SQLite-backed board state.
//!
//! Three processes touch this database at once — the engine daemon, the TUI, and
//! one-shot hook invocations — so every connection runs in WAL mode with a busy
//! timeout. The engine is the only writer that changes a card's lane as a result
//! of agent activity; the TUI and CLI only express intent.

pub mod cards;
pub mod migrations;
pub mod repos;
pub mod rules;
pub mod runs;

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::Connection;

/// One line of the board's audit trail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub at: i64,
    pub kind: String,
    pub card_id: Option<String>,
    pub detail: Option<String>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        Self::prepare(conn)
    }

    /// An ephemeral board, for tests and for `--dry-run` style experiments.
    pub fn open_in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrations::apply(&conn)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Run `f` in a transaction, rolling back on error.
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self.conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Newest `updated_at` across cards. The TUI polls this to decide whether to redraw.
    pub fn revision(&self) -> Result<i64> {
        let v: Option<i64> = self.conn.query_row(
            "SELECT MAX(m) FROM (
                SELECT MAX(updated_at) AS m FROM cards
                UNION ALL SELECT MAX(seq) FROM event_log
             )",
            [],
            |r| r.get(0),
        )?;
        Ok(v.unwrap_or(0))
    }

    pub fn log_event(&self, kind: &str, card_id: Option<&str>, detail: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO event_log (at, kind, card_id, detail) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![now(), kind, card_id, detail],
        )?;
        Ok(())
    }

    pub fn recent_events(&self, limit: u32) -> Result<Vec<LogEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT at, kind, card_id, detail FROM event_log ORDER BY seq DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |r| {
                Ok(LogEntry {
                    at: r.get(0)?,
                    kind: r.get(1)?,
                    card_id: r.get(2)?,
                    detail: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        let v = self
            .conn
            .query_row("SELECT v FROM kv WHERE k = ?1", [key], |r| r.get(0))
            .ok();
        Ok(v)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO kv (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            [key, value],
        )?;
        Ok(())
    }
}

/// Unix seconds. One helper so every timestamp in the database agrees.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// ULIDs carry a millisecond timestamp, so they sort roughly by creation — but
/// only roughly: two made inside the same millisecond are ordered by their
/// random tail. Anywhere creation order actually matters, order by `rowid`.
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string(v)?)
}

fn from_json<T: serde::de::DeserializeOwned>(s: &str) -> rusqlite::Result<T> {
    serde_json::from_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let store = Store::open_in_memory().unwrap();
        // Re-applying against the same connection must be a no-op, not an error.
        migrations::apply(store.conn()).unwrap();
        let v: i64 = store
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, migrations::latest_version());
    }

    #[test]
    fn a_newer_database_is_refused_rather_than_corrupted() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 999i64).unwrap();
        let err = migrations::apply(&conn).unwrap_err();
        assert!(err.to_string().contains("upgrade herdr-code-board"));
    }

    #[test]
    fn kv_round_trips() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.kv_get("missing").unwrap(), None);
        store.kv_set("engine_pid", "42").unwrap();
        store.kv_set("engine_pid", "43").unwrap();
        assert_eq!(store.kv_get("engine_pid").unwrap().as_deref(), Some("43"));
    }
}
