-- Recent searches shared by the phone and the PC (`recent.rs`). One row per entry key
-- (`q:<normalised text>` or `<kind>:<id>`): the item's JSON and when it was last used (Unix ms),
-- or `item = NULL` for a removal that must reach the other device (a tombstone). A clear is a
-- watermark in `settings` ('recent_cleared_at').
CREATE TABLE recent_search (
    key  TEXT PRIMARY KEY,
    at   INTEGER NOT NULL,
    item TEXT
);
