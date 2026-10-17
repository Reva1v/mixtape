-- YouTube's integrated loudness of the track, LUFS (`player` → audioConfig.trackAbsoluteLoudnessLkfs).
-- Known before any audio is on disk, so a streamed track is normalised from its first
-- sample; a measured EBU R128 value in media_files wins once the file exists.
ALTER TABLE tracks ADD COLUMN loudness_lufs REAL;
