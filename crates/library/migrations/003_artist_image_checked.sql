-- When the artist picture was last looked for (Unix ms). Artists whose page has no
-- picture, or whose fetch failed, wait a week before the next try.
ALTER TABLE artists ADD COLUMN image_checked_at INTEGER;
