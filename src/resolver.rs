//! Book resolution: cross-reference user input against Open Library so only
//! real books (with a valid ISBN) are ever stored.

use std::time::Duration;

use serde::Deserialize;

use crate::isbn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBook {
    pub title: String,
    pub isbn13: String,
    pub author: Option<String>,
}

/// A search suggestion shown to the user before they commit to a book.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BookSuggestion {
    pub title: String,
    pub author: Option<String>,
    pub isbn13: String,
    pub year: Option<i64>,
    /// Open Library cover id, usable as
    /// `https://covers.openlibrary.org/b/id/{id}-M.jpg`.
    pub cover_id: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    #[error("upstream book service unavailable")]
    Upstream(#[source] anyhow::Error),
}

#[async_trait::async_trait]
pub trait BookResolver: Send + Sync {
    /// Look up a book by validated ISBN-13. `None` if no such book exists.
    async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError>;
    /// Search for a book by title. `None` if nothing matches.
    async fn by_title(&self, title: &str) -> Result<Option<ResolvedBook>, ResolverError>;
    /// Typeahead search: several candidate books with author/cover metadata.
    async fn search(&self, query: &str) -> Result<Vec<BookSuggestion>, ResolverError>;
}

pub struct OpenLibraryResolver {
    client: reqwest::Client,
    base_url: String,
}

impl OpenLibraryResolver {
    pub fn new() -> Self {
        Self::with_base_url("https://openlibrary.org")
    }

    pub fn with_base_url(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            // Open Library asks API clients to identify themselves with a
            // contact in the User-Agent.
            .user_agent(concat!(
                "anonbookrec/",
                env!("CARGO_PKG_VERSION"),
                " (+https://suggestathing.com)"
            ))
            .build()
            .expect("reqwest client");
        Self { client, base_url: base_url.trim_end_matches('/').to_string() }
    }
}

impl Default for OpenLibraryResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct EditionResponse {
    title: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    docs: Vec<SearchDoc>,
}

#[derive(Deserialize)]
struct SearchDoc {
    title: Option<String>,
    isbn: Option<Vec<String>>,
    author_name: Option<Vec<String>>,
    first_publish_year: Option<i64>,
    cover_i: Option<i64>,
}

impl SearchDoc {
    /// First checksum-valid ISBN, normalized to ISBN-13.
    fn valid_isbn13(&self) -> Option<String> {
        self.isbn
            .as_deref()?
            .iter()
            .filter_map(|raw| isbn::looks_like_isbn(raw))
            .find_map(|cleaned| isbn::normalize(&cleaned))
    }
}

/// Resolves against a local SQLite database built from the monthly Open
/// Library dumps (see `src/bin/build_bookdb.rs`), so lookups of known books
/// never leave the machine. The builder writes rows in popularity order
/// (ratings + reading-log counts), so rowid order *is* the ranking — famous
/// books surface first with no sorting at query time.
pub struct DumpResolver {
    pool: sqlx::SqlitePool,
}

impl DumpResolver {
    pub async fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .read_only(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        // Fail at startup, not on first request, if the file isn't a book db.
        sqlx::query("SELECT 1 FROM books LIMIT 1").fetch_optional(&pool).await?;
        Ok(Self { pool })
    }

    /// Build an FTS5 MATCH expression from free text: every token quoted (so
    /// user input can't inject FTS syntax), the last one a prefix match.
    fn fts_query(input: &str) -> Option<String> {
        let tokens: Vec<String> = input
            .split_whitespace()
            .take(8)
            .map(|t| t.replace('"', ""))
            .filter(|t| !t.is_empty())
            .map(|t| format!("\"{t}\""))
            .collect();
        if tokens.is_empty() {
            return None;
        }
        let last = tokens.len() - 1;
        Some(
            tokens
                .iter()
                .enumerate()
                .map(|(i, t)| if i == last { format!("{t}*") } else { t.clone() })
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

type BookRow = (String, String, Option<String>, Option<i64>);

#[async_trait::async_trait]
impl BookResolver for DumpResolver {
    async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        let row = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT title, author FROM books WHERE isbn13 = ?
             ORDER BY id LIMIT 1",
        )
        .bind(isbn13)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ResolverError::Upstream(e.into()))?;
        Ok(row.map(|(title, author)| ResolvedBook {
            title,
            isbn13: isbn13.to_string(),
            author,
        }))
    }

    async fn by_title(&self, title: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        let Some(fts) = Self::fts_query(title) else { return Ok(None) };
        let row = sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT b.isbn13, b.title, b.author
             FROM books_fts f JOIN books b ON b.id = f.rowid
             WHERE books_fts MATCH ?
             ORDER BY f.rowid LIMIT 1",
        )
        .bind(&fts)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ResolverError::Upstream(e.into()))?;
        Ok(row.map(|(isbn13, title, author)| ResolvedBook { title, isbn13, author }))
    }

    async fn search(&self, query: &str) -> Result<Vec<BookSuggestion>, ResolverError> {
        let Some(fts) = Self::fts_query(query) else { return Ok(Vec::new()) };
        let rows = sqlx::query_as::<_, BookRow>(
            "SELECT b.isbn13, b.title, b.author, b.cover
             FROM books_fts f JOIN books b ON b.id = f.rowid
             WHERE books_fts MATCH ?
             ORDER BY f.rowid LIMIT 20",
        )
        .bind(&fts)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ResolverError::Upstream(e.into()))?;
        // Different editions of one work share a title; keep one per
        // title+author so five slots aren't five copies of the same book.
        let mut seen = std::collections::HashSet::new();
        Ok(rows
            .into_iter()
            .filter(|(_, title, author, _)| {
                seen.insert((title.to_lowercase(), author.clone().unwrap_or_default().to_lowercase()))
            })
            .map(|(isbn13, title, author, cover_id)| BookSuggestion {
                title,
                author,
                isbn13,
                year: None,
                cover_id,
            })
            .take(5)
            .collect())
    }
}

/// Tries the local dump first and falls back to the network resolver for
/// books the (monthly, or filtered) dump doesn't know.
pub struct ChainResolver {
    primary: std::sync::Arc<dyn BookResolver>,
    fallback: std::sync::Arc<dyn BookResolver>,
}

impl ChainResolver {
    pub fn new(
        primary: std::sync::Arc<dyn BookResolver>,
        fallback: std::sync::Arc<dyn BookResolver>,
    ) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait::async_trait]
impl BookResolver for ChainResolver {
    async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        if let Some(hit) = self.primary.by_isbn(isbn13).await? {
            return Ok(Some(hit));
        }
        self.fallback.by_isbn(isbn13).await
    }

    async fn by_title(&self, title: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        if let Some(hit) = self.primary.by_title(title).await? {
            return Ok(Some(hit));
        }
        self.fallback.by_title(title).await
    }

    async fn search(&self, query: &str) -> Result<Vec<BookSuggestion>, ResolverError> {
        let hits = self.primary.search(query).await?;
        if !hits.is_empty() {
            return Ok(hits);
        }
        self.fallback.search(query).await
    }
}

/// Serializes calls to the wrapped resolver and enforces a minimum interval
/// between them — one polite client toward Open Library no matter how many
/// concurrent visitors, per their API guidelines.
pub struct ThrottledResolver {
    inner: std::sync::Arc<dyn BookResolver>,
    min_interval: Duration,
    last: tokio::sync::Mutex<Option<tokio::time::Instant>>,
}

impl ThrottledResolver {
    pub fn new(inner: std::sync::Arc<dyn BookResolver>, min_interval: Duration) -> Self {
        Self { inner, min_interval, last: tokio::sync::Mutex::new(None) }
    }

    /// Hold the slot for the duration of the call, waiting out the interval.
    async fn slot(&self) -> tokio::sync::MutexGuard<'_, Option<tokio::time::Instant>> {
        let mut last = self.last.lock().await;
        if let Some(prev) = *last {
            tokio::time::sleep_until(prev + self.min_interval).await;
        }
        *last = Some(tokio::time::Instant::now());
        last
    }
}

#[async_trait::async_trait]
impl BookResolver for ThrottledResolver {
    async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        let _slot = self.slot().await;
        self.inner.by_isbn(isbn13).await
    }

    async fn by_title(&self, title: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        let _slot = self.slot().await;
        self.inner.by_title(title).await
    }

    async fn search(&self, query: &str) -> Result<Vec<BookSuggestion>, ResolverError> {
        let _slot = self.slot().await;
        self.inner.search(query).await
    }
}

/// Caches resolved books in SQLite so repeat submissions of known ISBNs never
/// touch Open Library, and title hits warm the ISBN cache. Search (typeahead)
/// is deliberately uncached — variety matters there.
pub struct CachingResolver {
    inner: std::sync::Arc<dyn BookResolver>,
    pool: sqlx::SqlitePool,
}

impl CachingResolver {
    pub fn new(inner: std::sync::Arc<dyn BookResolver>, pool: sqlx::SqlitePool) -> Self {
        Self { inner, pool }
    }

    async fn cache_get(&self, isbn13: &str) -> Option<ResolvedBook> {
        sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT title, author FROM book_cache WHERE isbn = ?",
        )
        .bind(isbn13)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(|(title, author)| ResolvedBook { title, isbn13: isbn13.to_string(), author })
    }

    async fn cache_put(&self, book: &ResolvedBook) {
        let _ = sqlx::query(
            "INSERT INTO book_cache (isbn, title, author) VALUES (?, ?, ?)
             ON CONFLICT(isbn) DO UPDATE SET title = excluded.title, author = excluded.author",
        )
        .bind(&book.isbn13)
        .bind(&book.title)
        .bind(&book.author)
        .execute(&self.pool)
        .await;
    }
}

#[async_trait::async_trait]
impl BookResolver for CachingResolver {
    async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        if let Some(hit) = self.cache_get(isbn13).await {
            return Ok(Some(hit));
        }
        let resolved = self.inner.by_isbn(isbn13).await?;
        if let Some(book) = &resolved {
            self.cache_put(book).await;
        }
        Ok(resolved)
    }

    async fn by_title(&self, title: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        let resolved = self.inner.by_title(title).await?;
        if let Some(book) = &resolved {
            self.cache_put(book).await;
        }
        Ok(resolved)
    }

    async fn search(&self, query: &str) -> Result<Vec<BookSuggestion>, ResolverError> {
        self.inner.search(query).await
    }
}

#[async_trait::async_trait]
impl BookResolver for OpenLibraryResolver {
    async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        // The search index carries author names in one request; prefer it,
        // and fall back to the edition endpoint (title only) for ISBNs that
        // exist but haven't been indexed.
        let url = format!("{}/search.json", self.base_url);
        let search: SearchResponse = self
            .client
            .get(&url)
            .query(&[
                ("q", format!("isbn:{isbn13}").as_str()),
                ("fields", "title,author_name"),
                ("limit", "1"),
            ])
            .send()
            .await
            .map_err(|e| ResolverError::Upstream(e.into()))?
            .error_for_status()
            .map_err(|e| ResolverError::Upstream(e.into()))?
            .json()
            .await
            .map_err(|e| ResolverError::Upstream(e.into()))?;
        if let Some(doc) = search.docs.into_iter().next()
            && let Some(title) = doc.title
        {
            return Ok(Some(ResolvedBook {
                title,
                isbn13: isbn13.to_string(),
                author: doc.author_name.and_then(|a| a.into_iter().next()),
            }));
        }

        let url = format!("{}/isbn/{}.json", self.base_url, isbn13);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ResolverError::Upstream(e.into()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let edition: EditionResponse = response
            .error_for_status()
            .map_err(|e| ResolverError::Upstream(e.into()))?
            .json()
            .await
            .map_err(|e| ResolverError::Upstream(e.into()))?;
        Ok(Some(ResolvedBook {
            title: edition.title,
            isbn13: isbn13.to_string(),
            author: None,
        }))
    }

    async fn by_title(&self, title: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        let url = format!("{}/search.json", self.base_url);
        let response: SearchResponse = self
            .client
            .get(&url)
            .query(&[("q", title), ("fields", "title,isbn,author_name"), ("limit", "5")])
            .send()
            .await
            .map_err(|e| ResolverError::Upstream(e.into()))?
            .error_for_status()
            .map_err(|e| ResolverError::Upstream(e.into()))?
            .json()
            .await
            .map_err(|e| ResolverError::Upstream(e.into()))?;

        // First search hit that carries at least one checksum-valid ISBN wins.
        for doc in response.docs {
            let Some(isbn13) = doc.valid_isbn13() else { continue };
            if let Some(title) = doc.title {
                return Ok(Some(ResolvedBook {
                    title,
                    isbn13,
                    author: doc.author_name.and_then(|a| a.into_iter().next()),
                }));
            }
        }
        Ok(None)
    }

    async fn search(&self, query: &str) -> Result<Vec<BookSuggestion>, ResolverError> {
        let url = format!("{}/search.json", self.base_url);
        let response: SearchResponse = self
            .client
            .get(&url)
            .query(&[
                ("q", query),
                ("fields", "title,isbn,author_name,first_publish_year,cover_i"),
                ("limit", "8"),
            ])
            .send()
            .await
            .map_err(|e| ResolverError::Upstream(e.into()))?
            .error_for_status()
            .map_err(|e| ResolverError::Upstream(e.into()))?
            .json()
            .await
            .map_err(|e| ResolverError::Upstream(e.into()))?;

        let suggestions = response
            .docs
            .into_iter()
            .filter_map(|doc| {
                let isbn13 = doc.valid_isbn13()?;
                Some(BookSuggestion {
                    title: doc.title?,
                    author: doc.author_name.and_then(|a| a.into_iter().next()),
                    isbn13,
                    year: doc.first_publish_year,
                    cover_id: doc.cover_i,
                })
            })
            .take(5)
            .collect();
        Ok(suggestions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    async fn fixture_dump() -> DumpResolver {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE books (
                 id INTEGER PRIMARY KEY, isbn13 TEXT NOT NULL,
                 title TEXT NOT NULL, author TEXT, cover INTEGER,
                 pop INTEGER NOT NULL DEFAULT 0
             );
             CREATE VIRTUAL TABLE books_fts USING fts5(
                 title, author, content = '',
                 tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3'
             );
             INSERT INTO books VALUES
                 (1, '9780134685991', 'Effective Java', 'Joshua Bloch', 111, 500),
                 (2, '9780439420891', 'The Dispossessed', 'Ursula K. Le Guin', 222, 90),
                 (3, '9780306406157', 'The Dispossessed', 'Ursula K. Le Guin', NULL, 90),
                 (4, '9781111111113', 'Dispossessed Farmers of the Dust Bowl', NULL, NULL, 0);
             INSERT INTO books_fts (rowid, title, author) VALUES
                 (1, 'Effective Java', 'Joshua Bloch'),
                 (2, 'The Dispossessed', 'Ursula K. Le Guin'),
                 (3, 'The Dispossessed', 'Ursula K. Le Guin'),
                 (4, 'Dispossessed Farmers of the Dust Bowl', NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
        DumpResolver { pool }
    }

    #[tokio::test]
    async fn dump_by_isbn_and_title() {
        let dump = fixture_dump().await;
        let hit = dump.by_isbn("9780134685991").await.unwrap().unwrap();
        assert_eq!(hit.title, "Effective Java");
        assert_eq!(hit.author.as_deref(), Some("Joshua Bloch"));
        assert!(dump.by_isbn("9780000000000").await.unwrap().is_none());

        let hit = dump.by_title("effective jav").await.unwrap().unwrap();
        assert_eq!(hit.isbn13, "9780134685991");
    }

    #[tokio::test]
    async fn dump_search_ranks_by_popularity_and_dedupes_editions() {
        let dump = fixture_dump().await;
        let hits = dump.search("dispossessed").await.unwrap();
        // Two editions of the popular work collapse into one suggestion (the
        // cover-bearing one, which the builder writes first), ranked above the
        // obscure literal match.
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].isbn13, "9780439420891");
        assert_eq!(hits[0].cover_id, Some(222));
        assert_eq!(hits[1].isbn13, "9781111111113");
    }

    #[tokio::test]
    async fn fts_query_quotes_user_input() {
        // FTS operators in user input must not reach MATCH unquoted.
        assert_eq!(DumpResolver::fts_query("harry AND \"pott"), Some("\"harry\" \"AND\" \"pott\"*".into()));
        assert_eq!(DumpResolver::fts_query("  "), None);
        let dump = fixture_dump().await;
        assert!(dump.search("NEAR( OR *").await.unwrap().is_empty());
    }

    struct CountingStub(AtomicUsize);

    #[async_trait::async_trait]
    impl BookResolver for CountingStub {
        async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Some(ResolvedBook {
                title: "Fallback Book".into(),
                isbn13: isbn13.into(),
                author: None,
            }))
        }
        async fn by_title(&self, _: &str) -> Result<Option<ResolvedBook>, ResolverError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
        async fn search(&self, _: &str) -> Result<Vec<BookSuggestion>, ResolverError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn chain_prefers_dump_and_falls_back() {
        let counter = Arc::new(CountingStub(AtomicUsize::new(0)));
        let chain = ChainResolver::new(Arc::new(fixture_dump().await), counter.clone());
        // Known ISBN: dump answers, fallback untouched.
        let hit = chain.by_isbn("9780134685991").await.unwrap().unwrap();
        assert_eq!(hit.title, "Effective Java");
        assert_eq!(counter.0.load(Ordering::SeqCst), 0);
        // Unknown ISBN: fallback consulted.
        let hit = chain.by_isbn("9780000000000").await.unwrap().unwrap();
        assert_eq!(hit.title, "Fallback Book");
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_spaces_out_calls() {
        let counter = Arc::new(CountingStub(AtomicUsize::new(0)));
        let throttled =
            Arc::new(ThrottledResolver::new(counter.clone(), Duration::from_secs(1)));
        let start = tokio::time::Instant::now();
        let (a, b) = tokio::join!(
            throttled.by_isbn("9780134685991"),
            throttled.by_isbn("9780306406157")
        );
        a.unwrap();
        b.unwrap();
        // Second call had to wait out the interval (virtual time).
        assert!(start.elapsed() >= Duration::from_secs(1));
        assert_eq!(counter.0.load(Ordering::SeqCst), 2);
    }
}
