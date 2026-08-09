CREATE TABLE users (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    slug            TEXT NOT NULL UNIQUE,
    secret_hash     TEXT NOT NULL,
    public_key      BLOB NOT NULL,
    enc_private_key BLOB NOT NULL,
    default_public  INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE recommendations (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id              INTEGER REFERENCES users(id) ON DELETE CASCADE,
    book_name            TEXT,
    isbn                 TEXT,
    enc_payload          BLOB,
    isbn_index           TEXT NOT NULL,
    user_key             TEXT NOT NULL,
    timestamp            TEXT NOT NULL DEFAULT (datetime('now')),
    is_public            INTEGER NOT NULL DEFAULT 1,
    recommendation_count INTEGER NOT NULL DEFAULT 1,
    read                 INTEGER NOT NULL DEFAULT 0,
    -- A row is either plaintext (public) or sealed (private), never both.
    CHECK (
        (enc_payload IS NULL AND book_name IS NOT NULL AND isbn IS NOT NULL)
        OR (enc_payload IS NOT NULL AND book_name IS NULL AND isbn IS NULL)
    )
);

CREATE UNIQUE INDEX uniq_page_isbn ON recommendations (coalesce(user_id, 0), isbn_index);
CREATE INDEX idx_recs_user ON recommendations (user_id);

CREATE TABLE recommendation_votes (
    recommendation_id INTEGER NOT NULL REFERENCES recommendations(id) ON DELETE CASCADE,
    submitter_hash    TEXT NOT NULL,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (recommendation_id, submitter_hash)
);
