-- Optional public profile. Shown to visitors only while the page is public
-- (default_public = 1).
ALTER TABLE users ADD COLUMN bio TEXT;
ALTER TABLE users ADD COLUMN favourite_book TEXT;
ALTER TABLE users ADD COLUMN theme TEXT;
