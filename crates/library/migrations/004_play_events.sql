-- Listening log for the statistics screen: one row per play of at least 30 s of real
-- playback (seeks and pauses not counted). Starts empty: play_stats has no times to
-- rebuild it from.
CREATE TABLE play_events (
    id          INTEGER PRIMARY KEY,
    track_id    TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    -- When the play became audible (Unix ms).
    started_at  INTEGER NOT NULL,
    listened_ms INTEGER NOT NULL
);
-- Period scans read only this index (covering).
CREATE INDEX play_events_time ON play_events(started_at, track_id, listened_ms);
-- Cascading deletes and twin merges look events up by track.
CREATE INDEX play_events_track ON play_events(track_id);
