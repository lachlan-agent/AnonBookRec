#!/usr/bin/env bash
# Rebuild the local book database from the monthly Open Library dumps and ship
# it to the droplet. Run from the repo root, roughly once a month (the dumps
# are regenerated monthly): https://openlibrary.org/developers/dumps
#
#   ./deploy/update-bookdb.sh [user@host]
#
# Requires ~15 GB free locally for the dumps. The droplet only ever holds the
# finished SQLite file (streamed gzipped, unpacked into the app-data volume).
set -euo pipefail

HOST="${1:-root@168.144.161.64}"
DUMPS=dumps
DB=books.sqlite

mkdir -p "$DUMPS"
echo "==> downloading dumps (skipped if already present)"
for part in authors editions; do
    [ -f "$DUMPS/$part.txt.gz" ] || \
        curl -L -o "$DUMPS/$part.txt.gz" \
            "https://openlibrary.org/data/ol_dump_${part}_latest.txt.gz"
done

echo "==> building $DB (covers-only keeps it small enough for the droplet)"
rm -f "$DB"
cargo run --release --bin build_bookdb -- \
    "$DUMPS/authors.txt.gz" "$DUMPS/editions.txt.gz" "$DB" --covers-only

echo "==> shipping to $HOST"
# Stream into the named volume via a helper container: the app mounts the
# volume at /data and picks the file up on restart.
gzip -c "$DB" | ssh "$HOST" '
    set -e
    gunzip -c | docker run --rm -i -v anonbookrec_app-data:/data debian:trixie-slim \
        sh -c "cat > /data/books.sqlite.new && mv /data/books.sqlite.new /data/books.sqlite && chown 999:999 /data/books.sqlite"
    cd /opt/anonbookrec && docker compose restart app
'

echo "==> done"
