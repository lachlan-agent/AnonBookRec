-- Optional username + password login. The password unlocks enc_secret (the
-- page secret encrypted under an Argon2id-derived key); the secret itself
-- remains the real credential and the backup key.
ALTER TABLE users ADD COLUMN username TEXT;
ALTER TABLE users ADD COLUMN password_hash TEXT;
ALTER TABLE users ADD COLUMN enc_secret BLOB;

CREATE UNIQUE INDEX uniq_users_username ON users (username) WHERE username IS NOT NULL;
