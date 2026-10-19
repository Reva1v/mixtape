-- Smart transitions: tempo, beat grid, key and structure of a track, analysed once from its
-- audio file in the background. `version` is the analyser's; older rows are redone.
-- `end_ms` 0 marks a track that could not be analysed (it is not retried until the version
-- changes).
CREATE TABLE track_analysis (
    track_id       TEXT PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    version        INTEGER NOT NULL,
    bpm            REAL NOT NULL,
    beat_ms        REAL NOT NULL,
    key            INTEGER,
    start_ms       INTEGER NOT NULL,
    end_ms         INTEGER NOT NULL,
    intro_end_ms   INTEGER NOT NULL,
    outro_start_ms INTEGER NOT NULL,
    analyzed_at    INTEGER NOT NULL
);
