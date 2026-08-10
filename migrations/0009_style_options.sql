-- Presentation options move to their own table: `users` stays a lean
-- identity/auth/crypto table. One row per user, created on first customization.
-- All values are whitelisted enums or a validated hex colour — never raw CSS.
CREATE TABLE user_theme (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    theme   TEXT,
    font    TEXT,
    accent  TEXT,
    radius  TEXT,
    density TEXT
);

INSERT INTO user_theme (user_id, theme)
    SELECT id, theme FROM users WHERE theme IS NOT NULL;

ALTER TABLE users DROP COLUMN theme;
