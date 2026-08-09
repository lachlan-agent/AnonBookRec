-- Optional per-submitter note ("I liked this because…"). Plaintext when the
-- recommendation is public, sealed to the page key when private.
ALTER TABLE recommendation_votes ADD COLUMN note TEXT;
ALTER TABLE recommendation_votes ADD COLUMN enc_note BLOB;
