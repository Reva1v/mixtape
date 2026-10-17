-- Last.fm scrobbles waiting to be sent, oldest first. The track's names are copied in, so
-- a scrobble survives the track leaving the library. One row per start time: a play is
-- never queued twice.
CREATE TABLE scrobble_outbox (
    id         INTEGER PRIMARY KEY,
    artist     TEXT NOT NULL,
    track      TEXT NOT NULL,
    album      TEXT,
    duration_s INTEGER,
    -- When the play started (Unix seconds), the scrobble's timestamp.
    started_at INTEGER NOT NULL UNIQUE,
    attempts   INTEGER NOT NULL DEFAULT 0
);
