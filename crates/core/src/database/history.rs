use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::Result;

use super::{lock_connection, Database};

#[derive(Debug, Serialize, Deserialize)]
pub struct StringRevision {
    pub id: i64,
    pub entry_id: String,
    pub old_translation: Option<String>,
    pub new_translation: Option<String>,
    pub old_status: Option<String>,
    pub new_status: Option<String>,
    pub provider: Option<String>,
    pub changed_at: String,
}

impl Database {
    /// Return the latest revisions, retaining at most 50 per string.
    pub fn get_string_history(&self, entry_id: &str, limit: usize) -> Result<Vec<StringRevision>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT id, entry_id, old_translation, new_translation, old_status, new_status,
                    provider, changed_at
             FROM string_revisions WHERE entry_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![entry_id, limit.min(50) as i64], |row| {
            Ok(StringRevision {
                id: row.get(0)?,
                entry_id: row.get(1)?,
                old_translation: row.get(2)?,
                new_translation: row.get(3)?,
                old_status: row.get(4)?,
                new_status: row.get(5)?,
                provider: row.get(6)?,
                changed_at: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}
