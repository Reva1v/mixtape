//! Key/value settings stored as JSON in the `settings` table.

use crate::{Db, Result};
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub fn get<T: DeserializeOwned>(db: &Db, key: &str) -> Result<Option<T>> {
    let raw: Option<String> = db.read(|c| {
        Ok(c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    })?;
    raw.map(|s| serde_json::from_str(&s)).transpose().map_err(Into::into)
}

pub fn get_or<T: DeserializeOwned>(db: &Db, key: &str, default: T) -> Result<T> {
    Ok(get(db, key)?.unwrap_or(default))
}

pub fn set<T: Serialize>(db: &Db, key: &str, value: &T) -> Result<()> {
    let key = key.to_owned();
    let json = serde_json::to_string(value)?;
    db.write(move |t| {
        t.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (&key, &json),
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        assert_eq!(get::<f32>(&db, "volume").unwrap(), None);
        set(&db, "volume", &0.5f32).unwrap();
        set(&db, "volume", &0.75f32).unwrap();
        assert_eq!(get::<f32>(&db, "volume").unwrap(), Some(0.75));
        assert_eq!(get_or(&db, "missing", 3u32).unwrap(), 3);
    }
}
