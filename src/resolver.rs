//! Book resolution: cross-reference user input against Open Library so only
//! real books (with a valid ISBN) are ever stored.

use std::time::Duration;

use serde::Deserialize;

use crate::isbn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBook {
    pub title: String,
    pub isbn13: String,
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
}

#[async_trait::async_trait]
impl BookResolver for OpenLibraryResolver {
    async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError> {
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
        Ok(Some(ResolvedBook { title: edition.title, isbn13: isbn13.to_string() }))
    }

    async fn by_title(&self, title: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        let url = format!("{}/search.json", self.base_url);
        let response: SearchResponse = self
            .client
            .get(&url)
            .query(&[("q", title), ("fields", "title,isbn"), ("limit", "5")])
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
            let (Some(doc_title), Some(isbns)) = (doc.title, doc.isbn) else { continue };
            let isbn13 = isbns
                .iter()
                .filter_map(|raw| isbn::looks_like_isbn(raw))
                .find_map(|cleaned| isbn::normalize(&cleaned));
            if let Some(isbn13) = isbn13 {
                return Ok(Some(ResolvedBook { title: doc_title, isbn13 }));
            }
        }
        Ok(None)
    }
}
