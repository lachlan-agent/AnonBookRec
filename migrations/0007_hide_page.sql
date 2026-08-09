-- Allow hiding pages from public view. Login Required to view a hidden page.
-- Hidden page users may generate a randomized long slug to share with friends, 
-- but the page will not be discoverable by search engines or other users.

ALTER TABLE users ADD COLUMN is_hidden INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN share_token TEXT;

CREATE UNIQUE INDEX uniq_users_share_token ON users (share_token)
    WHERE share_token IS NOT NULL;
