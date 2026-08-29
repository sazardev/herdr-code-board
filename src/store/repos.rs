//! Repo records: the tagged git checkouts the board dispatches into.

use anyhow::Result;
use rusqlite::{params, Row};

use super::{from_json, new_id, to_json, Store};
use crate::model::Repo;

fn row_to_repo(r: &Row<'_>) -> rusqlite::Result<Repo> {
    Ok(Repo {
        id: r.get("id")?,
        name: r.get("name")?,
        path: r.get("path")?,
        tags: from_json(&r.get::<_, String>("tags")?)?,
        max_parallel: r.get::<_, i64>("max_parallel")? as u32,
        default_agent: r.get("default_agent")?,
        default_model: r.get("default_model")?,
    })
}

const SELECT: &str =
    "SELECT id, name, path, tags, max_parallel, default_agent, default_model FROM repos";

impl Store {
    pub fn list_repos(&self) -> Result<Vec<Repo>> {
        let mut stmt = self.conn().prepare(&format!("{SELECT} ORDER BY name"))?;
        let rows = stmt
            .query_map([], row_to_repo)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn get_repo(&self, id: &str) -> Result<Option<Repo>> {
        let mut stmt = self.conn().prepare(&format!("{SELECT} WHERE id = ?1"))?;
        let mut rows = stmt.query_map([id], row_to_repo)?;
        Ok(rows.next().transpose()?)
    }

    pub fn find_repo_by_path(&self, path: &str) -> Result<Option<Repo>> {
        let mut stmt = self.conn().prepare(&format!("{SELECT} WHERE path = ?1"))?;
        let mut rows = stmt.query_map([path], row_to_repo)?;
        Ok(rows.next().transpose()?)
    }

    /// Resolve a repo by id, exact path, or name. Used everywhere a human types one.
    pub fn resolve_repo(&self, needle: &str) -> Result<Option<Repo>> {
        if let Some(r) = self.get_repo(needle)? {
            return Ok(Some(r));
        }
        if let Some(r) = self.find_repo_by_path(needle)? {
            return Ok(Some(r));
        }
        let mut stmt = self.conn().prepare(&format!("{SELECT} WHERE name = ?1"))?;
        let mut rows = stmt.query_map([needle], row_to_repo)?;
        Ok(rows.next().transpose()?)
    }

    /// Insert or update by path, which is the repo's natural key.
    pub fn upsert_repo(&self, repo: &Repo) -> Result<Repo> {
        let existing = self.find_repo_by_path(&repo.path)?;
        let id = existing
            .as_ref()
            .map(|r| r.id.clone())
            .unwrap_or_else(new_id);
        self.conn().execute(
            "INSERT INTO repos (id, name, path, tags, max_parallel, default_agent, default_model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET
                name = excluded.name,
                tags = excluded.tags,
                max_parallel = excluded.max_parallel,
                default_agent = excluded.default_agent,
                default_model = excluded.default_model",
            params![
                id,
                repo.name,
                repo.path,
                to_json(&repo.tags)?,
                repo.max_parallel as i64,
                repo.default_agent,
                repo.default_model,
            ],
        )?;
        Ok(Repo { id, ..repo.clone() })
    }

    pub fn delete_repo(&self, id: &str) -> Result<bool> {
        let n = self
            .conn()
            .execute("DELETE FROM repos WHERE id = ?1", [id])?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str, path: &str) -> Repo {
        Repo {
            id: String::new(),
            name: name.into(),
            path: path.into(),
            tags: vec!["work".into()],
            max_parallel: 2,
            default_agent: None,
            default_model: None,
        }
    }

    #[test]
    fn upsert_keeps_the_id_stable_for_the_same_path() {
        let store = Store::open_in_memory().unwrap();
        let a = store.upsert_repo(&repo("erp", "/tmp/erp")).unwrap();
        let b = store
            .upsert_repo(&Repo {
                name: "erp-renamed".into(),
                max_parallel: 5,
                ..repo("erp", "/tmp/erp")
            })
            .unwrap();
        assert_eq!(a.id, b.id, "the same path must not create a second repo");
        let stored = store.get_repo(&a.id).unwrap().unwrap();
        assert_eq!(stored.name, "erp-renamed");
        assert_eq!(stored.max_parallel, 5);
        assert_eq!(store.list_repos().unwrap().len(), 1);
    }

    #[test]
    fn resolve_accepts_id_path_or_name() {
        let store = Store::open_in_memory().unwrap();
        let r = store.upsert_repo(&repo("erp", "/tmp/erp")).unwrap();
        for needle in [r.id.as_str(), "/tmp/erp", "erp"] {
            assert_eq!(store.resolve_repo(needle).unwrap().unwrap().id, r.id);
        }
        assert!(store.resolve_repo("nope").unwrap().is_none());
    }
}
