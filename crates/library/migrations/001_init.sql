-- Mixtape schema v1. See specs/001-mixtape-mvp/data-model.md.

CREATE TABLE artists (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    image_key   TEXT,
    in_library  INTEGER NOT NULL DEFAULT 0,
    synced_at   INTEGER,
    search_text TEXT NOT NULL DEFAULT ''
);

CREATE TABLE albums (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    year        INTEGER,
    album_type  TEXT NOT NULL DEFAULT 'album' CHECK (album_type IN ('album', 'ep', 'single')),
    cover_key   TEXT,
    palette     TEXT,
    in_library  INTEGER NOT NULL DEFAULT 0,
    track_count INTEGER,
    search_text TEXT NOT NULL DEFAULT ''
);

CREATE TABLE album_artists (
    album_id  TEXT NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    artist_id TEXT REFERENCES artists(id) ON DELETE SET NULL,
    name      TEXT NOT NULL,
    PRIMARY KEY (album_id, position)
);
CREATE INDEX album_artists_artist ON album_artists(artist_id);

CREATE TABLE tracks (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    album_id    TEXT REFERENCES albums(id) ON DELETE SET NULL,
    track_no    INTEGER,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    cover_key   TEXT,
    state       TEXT NOT NULL DEFAULT 'remote'
                CHECK (state IN ('remote', 'cached', 'downloaded', 'broken')),
    liked       INTEGER NOT NULL DEFAULT 0,
    liked_at    INTEGER,
    added_at    INTEGER,
    search_text TEXT NOT NULL DEFAULT ''
);
CREATE INDEX tracks_album ON tracks(album_id, track_no);
CREATE INDEX tracks_state_added ON tracks(state, added_at);
CREATE INDEX tracks_liked ON tracks(liked, liked_at);

CREATE TABLE track_artists (
    track_id  TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    artist_id TEXT REFERENCES artists(id) ON DELETE SET NULL,
    name      TEXT NOT NULL,
    PRIMARY KEY (track_id, position)
);
CREATE INDEX track_artists_artist ON track_artists(artist_id);

CREATE TABLE media_files (
    track_id    TEXT PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    container   TEXT NOT NULL CHECK (container IN ('webm', 'm4a')),
    itag        INTEGER,
    size_bytes  INTEGER NOT NULL,
    sample_rate INTEGER,
    loudness_db REAL,
    lufs        REAL,
    true_peak   REAL,
    last_access INTEGER NOT NULL,
    location    TEXT NOT NULL CHECK (location IN ('cache', 'library'))
);
CREATE INDEX media_files_lru ON media_files(location, last_access);

CREATE TABLE playlists (
    id        TEXT PRIMARY KEY,
    title     TEXT NOT NULL,
    origin    TEXT NOT NULL CHECK (origin IN ('ytm', 'local')),
    cover_key TEXT,
    synced_at INTEGER,
    search_text TEXT NOT NULL DEFAULT ''
);

CREATE TABLE playlist_items (
    playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    track_id    TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    PRIMARY KEY (playlist_id, position)
);
CREATE INDEX playlist_items_track ON playlist_items(track_id);

CREATE TABLE play_stats (
    track_id        TEXT PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    play_count      INTEGER NOT NULL DEFAULT 0,
    full_play_count INTEGER NOT NULL DEFAULT 0,
    last_played_at  INTEGER
);
CREATE INDEX play_stats_recent ON play_stats(last_played_at);

CREATE TABLE sync_sources (
    kind           TEXT PRIMARY KEY,
    enabled        INTEGER NOT NULL DEFAULT 1,
    last_synced_at INTEGER,
    last_error     TEXT
);

CREATE TABLE download_jobs (
    id          INTEGER PRIMARY KEY,
    track_id    TEXT NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    source      TEXT NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('queued', 'running', 'paused', 'done', 'failed')),
    bytes_done  INTEGER NOT NULL DEFAULT 0,
    bytes_total INTEGER,
    attempts    INTEGER NOT NULL DEFAULT 0,
    error       TEXT,
    updated_at  INTEGER NOT NULL
);
-- One active job per track.
CREATE UNIQUE INDEX download_jobs_active ON download_jobs(track_id)
    WHERE status IN ('queued', 'running', 'paused');
CREATE INDEX download_jobs_status ON download_jobs(status, id);

CREATE TABLE like_outbox (
    id         INTEGER PRIMARY KEY,
    track_id   TEXT NOT NULL UNIQUE,
    action     TEXT NOT NULL CHECK (action IN ('like', 'unlike')),
    created_at INTEGER NOT NULL,
    attempts   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE lyrics_cache (
    track_id   TEXT PRIMARY KEY,
    synced     INTEGER NOT NULL DEFAULT 0,
    body       TEXT,
    fetched_at INTEGER NOT NULL
);

-- Substring search over tracks. External content: rows live in `tracks`.
CREATE VIRTUAL TABLE tracks_fts USING fts5(
    search_text,
    tokenize = 'trigram',
    content = 'tracks',
    content_rowid = 'rowid'
);

CREATE TRIGGER tracks_fts_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, search_text) VALUES (new.rowid, new.search_text);
END;
CREATE TRIGGER tracks_fts_ad AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, search_text) VALUES ('delete', old.rowid, old.search_text);
END;
CREATE TRIGGER tracks_fts_au AFTER UPDATE OF search_text ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, search_text) VALUES ('delete', old.rowid, old.search_text);
    INSERT INTO tracks_fts(rowid, search_text) VALUES (new.rowid, new.search_text);
END;
