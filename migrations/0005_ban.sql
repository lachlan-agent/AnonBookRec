-- Optional blacklisting of books by ISB. User configured.

CREATE TABLE user_book_blacklist (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    isbn TEXT NOT NULL,
    UNIQUE (user_id, isbn)
);