-- Phone ↔ PC stats sync (specs 007): every play gets a global id, the device that logged it
-- and its sequence number there, so two logs merge by plain union.
--
-- This device's id: a random UUID v4, made here because the old rows need it. `open_writer`
-- runs the same insert after the migrations, in case the row is ever lost. Settings hold JSON.
INSERT OR IGNORE INTO settings(key, value) VALUES ('device_id', '"' ||
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + (abs(random()) % 4), 1) ||
    substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))) || '"');

-- Rebuilt rather than altered: NOT NULL columns, and AUTOINCREMENT so a deleted last row's id
-- is never handed out again (a local row's `seq` is its id, and a peer that already has that
-- seq would skip the new play).
CREATE TABLE play_events_new (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    track_id    TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    -- When the play became audible (Unix ms).
    started_at  INTEGER NOT NULL,
    listened_ms INTEGER NOT NULL,
    -- The device that logged the play (`settings.device_id` there) and the play's number on
    -- it: its `id` there. Only grows per device, so "what's new" is `seq > max seen`.
    device      TEXT NOT NULL,
    seq         INTEGER NOT NULL
);
INSERT INTO play_events_new (id, track_id, started_at, listened_ms, device, seq)
    SELECT id, track_id, started_at, listened_ms,
           (SELECT json_extract(value, '$') FROM settings WHERE key = 'device_id'), id
    FROM play_events;
DROP TABLE play_events;
ALTER TABLE play_events_new RENAME TO play_events;
-- Period scans read only this index (covering), with or without a device filter.
CREATE INDEX play_events_time ON play_events(started_at, track_id, listened_ms, device);
-- Cascading deletes and twin merges look events up by track.
CREATE INDEX play_events_track ON play_events(track_id);
-- The global id; also serves the per-device cursors (`MAX(seq) ... GROUP BY device`).
CREATE UNIQUE INDEX play_events_origin ON play_events(device, seq);
