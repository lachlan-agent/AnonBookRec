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
            .user_agent(concat!("anonbookrec/", env!("CARGO_PKG_VERSION")))
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
