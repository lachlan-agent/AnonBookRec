-- Author of the recommended book. Plaintext for public rows; private rows
-- carry it inside the sealed payload instead (column stays NULL).
ALTER TABLE recommendations ADD COLUMN author TEXT;
