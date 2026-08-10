-- Wire up the book blacklist (0005) with a display title, and add a local
-- cache of resolved books so repeat lookups skip Open Library.
ALTER TABLE user_book_blacklist ADD COLUMN title TEXT;

CREATE TABLE book_cache (
    isbn      TEXT PRIMARY KEY,
    title     TEXT NOT NULL,
    author    TEXT,
    cached_at TEXT NOT NULL DEFAULT (datetime('now'))
);
