//! Key/value settings stored as JSON in the `settings` table.

use crate::{Db, Result};
use rusqlite::{Connection, OptionalExtension};
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

/// This device's id in the play log (`play_events.device`): a UUID v4 the database makes on
/// its first open (migration 009, `db::open_writer`).
pub const DEVICE_ID: &str = "device_id";
/// The name other devices show for this one, set by the app (a string). See [`device_name`].
pub const DEVICE_NAME: &str = "device_name";

/// This device's id; see [`DEVICE_ID`].
pub fn device_id(conn: &Connection) -> Result<String> {
    let raw: String = conn.query_row("SELECT value FROM settings WHERE key = ?1", [DEVICE_ID], |r| r.get(0))?;
    Ok(serde_json::from_str(&raw)?)
}

/// The name of this device: the [`DEVICE_NAME`] setting, else the computer's name.
pub fn device_name(conn: &Connection) -> Result<String> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [DEVICE_NAME], |r| r.get(0))
        .optional()?;
    Ok(match raw.map(|s| serde_json::from_str::<String>(&s)).transpose()? {
        Some(name) if !name.trim().is_empty() => name,
        _ => default_device_name(),
    })
}

/// The computer's name from the environment (`COMPUTERNAME` on Windows, `HOSTNAME`
/// elsewhere), or "Mixtape". Android has neither: the app sets [`DEVICE_NAME`].
pub fn default_device_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|s| s.trim().to_owned())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "Mixtape".to_owned())
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

    #[test]
    fn device_name_falls_back_to_the_computer() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        assert_eq!(db.read(device_name).unwrap(), default_device_name());
        set(&db, DEVICE_NAME, &"Galaxy S23+").unwrap();
        assert_eq!(db.read(device_name).unwrap(), "Galaxy S23+");
        let id = db.read(device_id).unwrap();
        assert_eq!(get::<String>(&db, DEVICE_ID).unwrap(), Some(id));
    }
}
