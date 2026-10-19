-- SoundCloud / Bandcamp imports keep the file yt-dlp downloads as is: MP3, or Ogg Opus, next to
-- WebM and M4A. SQLite cannot change a CHECK in place, so the table is rebuilt (only a child
-- of `tracks`; nothing references it).
CREATE TABLE media_files_new (
    track_id    TEXT PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    container   TEXT NOT NULL CHECK (container IN ('webm', 'm4a', 'mp3', 'ogg')),
    itag        INTEGER,
    size_bytes  INTEGER NOT NULL,
    sample_rate INTEGER,
    loudness_db REAL,
    lufs        REAL,
    true_peak   REAL,
    last_access INTEGER NOT NULL,
    location    TEXT NOT NULL CHECK (location IN ('cache', 'library'))
);
INSERT INTO media_files_new (track_id, path, container, itag, size_bytes, sample_rate, loudness_db, lufs, true_peak, last_access, location)
    SELECT track_id, path, container, itag, size_bytes, sample_rate, loudness_db, lufs, true_peak, last_access, location
    FROM media_files;
DROP TABLE media_files;
ALTER TABLE media_files_new RENAME TO media_files;
CREATE INDEX media_files_lru ON media_files(location, last_access);
