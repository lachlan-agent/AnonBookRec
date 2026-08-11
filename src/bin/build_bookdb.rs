//! Build a local book database from the monthly Open Library dumps, so the
//! server can resolve ISBNs and serve typeahead without touching the Open
//! Library API for known books.
//!
//! ```sh
//! cargo run --release --bin build_bookdb -- \
//!     dumps/authors.txt.gz dumps/editions.txt.gz books.sqlite [--covers-only]
//! ```
//!
//! Dumps: <https://openlibrary.org/developers/dumps>. Each line is five
//! tab-separated columns: type, key, revision, last_modified, JSON.
//!
//! `--covers-only` keeps only editions that have a cover image — a quality
//! filter that shrinks the database several-fold for small hosts; books people
//! actually recommend almost always have covers.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};

use anonbookrec::isbn;
use flate2::read::MultiGzDecoder;
use serde::Deserialize;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Connection, SqliteConnection};

#[derive(Deserialize)]
struct AuthorRecord {
    key: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct EditionRecord {
    title: Option<String>,
    #[serde(default)]
    isbn_13: Vec<String>,
    #[serde(default)]
    isbn_10: Vec<String>,
    #[serde(default)]
    authors: Vec<KeyRef>,
    #[serde(default)]
    covers: Vec<Option<i64>>,
}

#[derive(Deserialize)]
struct KeyRef {
    key: Option<String>,
}

fn open_dump(path: &str) -> anyhow::Result<impl BufRead> {
    let file = std::fs::File::open(path)?;
    Ok(BufReader::with_capacity(1 << 20, MultiGzDecoder::new(file)))
}

/// The JSON document is the fifth tab-separated column.
fn json_column(line: &str) -> Option<&str> {
    line.splitn(5, '\t').nth(4)
}

fn load_authors(path: &str) -> anyhow::Result<HashMap<Box<str>, Box<str>>> {
    let mut authors: HashMap<Box<str>, Box<str>> = HashMap::new();
    let mut count = 0u64;
    for line in open_dump(path)?.lines() {
        let line = line?;
        let Some(json) = json_column(&line) else { continue };
        let Ok(record) = serde_json::from_str::<AuthorRecord>(json) else { continue };
        let Some(name) = record.name else { continue };
        let key = record.key.trim_start_matches("/authors/");
        authors.insert(key.into(), name.into());
        count += 1;
        if count.is_multiple_of(1_000_000) {
            eprintln!("authors: {count}");
        }
    }
    eprintln!("authors loaded: {}", authors.len());
    Ok(authors)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, authors_path, editions_path, out_path, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: build_bookdb <authors.txt.gz> <editions.txt.gz> <out.sqlite> [--covers-only]");
    };
    let covers_only = rest.iter().any(|a| a == "--covers-only");

    if std::path::Path::new(out_path).exists() {
        anyhow::bail!("{out_path} already exists; remove it first");
    }

    let authors = load_authors(authors_path)?;

    let options = SqliteConnectOptions::new()
        .filename(out_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Off)
        .synchronous(SqliteSynchronous::Off);
    let mut conn = SqliteConnection::connect_with(&options).await?;

    sqlx::raw_sql(
        "CREATE TABLE books (
             id      INTEGER PRIMARY KEY,
             isbn13  TEXT NOT NULL,
             title   TEXT NOT NULL,
             author  TEXT,
             cover   INTEGER
         );
         CREATE VIRTUAL TABLE books_fts USING fts5(
             title, author,
             content = '',
             tokenize = 'unicode61 remove_diacritics 2',
             prefix = '2 3'
         );",
    )
    .execute(&mut conn)
    .await?;

    let mut lines = 0u64;
    let mut kept = 0u64;
    let mut next_id: i64 = 1;
    let mut tx = conn.begin().await?;
    for line in open_dump(editions_path)?.lines() {
        let line = line?;
        lines += 1;
        if lines.is_multiple_of(5_000_000) {
            eprintln!("editions: {lines} scanned, {kept} kept");
        }
        let Some(json) = json_column(&line) else { continue };
        let Ok(record) = serde_json::from_str::<EditionRecord>(json) else { continue };
        let Some(title) = record.title else { continue };
        if title.is_empty() || title.len() > 512 {
            continue;
        }
        // First checksum-valid ISBN, normalized to 13.
        let Some(isbn13) = record
            .isbn_13
            .iter()
            .chain(record.isbn_10.iter())
            .filter_map(|raw| isbn::looks_like_isbn(raw))
            .find_map(|cleaned| isbn::normalize(&cleaned))
        else {
            continue;
        };
        let cover = record.covers.iter().flatten().copied().find(|&c| c > 0);
        if covers_only && cover.is_none() {
            continue;
        }
        let author = record
            .authors
            .iter()
            .filter_map(|a| a.key.as_deref())
            .map(|k| k.trim_start_matches("/authors/"))
            .find_map(|k| authors.get(k))
            .map(|name| name.to_string());

        sqlx::query("INSERT INTO books (id, isbn13, title, author, cover) VALUES (?, ?, ?, ?, ?)")
            .bind(next_id)
            .bind(&isbn13)
            .bind(&title)
            .bind(&author)
            .bind(cover)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO books_fts (rowid, title, author) VALUES (?, ?, ?)")
            .bind(next_id)
            .bind(&title)
            .bind(&author)
            .execute(&mut *tx)
            .await?;
        next_id += 1;
        kept += 1;
        if kept.is_multiple_of(100_000) {
            tx.commit().await?;
            tx = conn.begin().await?;
        }
    }
    tx.commit().await?;

    eprintln!("indexing isbn13…");
    sqlx::raw_sql(
        "CREATE INDEX idx_books_isbn ON books (isbn13);
         INSERT INTO books_fts (books_fts) VALUES ('optimize');",
    )
    .execute(&mut conn)
    .await?;
    sqlx::raw_sql("VACUUM").execute(&mut conn).await?;
    conn.close().await?;

    eprintln!("done: {kept} books from {lines} edition records -> {out_path}");
    Ok(())
}
