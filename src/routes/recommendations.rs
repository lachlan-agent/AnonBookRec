use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use axum_extra::extract::CookieJar;
use crypto_box::SecretKey;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;

use crate::auth::{self, User};
use crate::crypto;
use crate::error::AppError;
use crate::isbn;
use crate::state::AppState;

const MAX_INPUT_LEN: usize = 256;

/// Plaintext content of a sealed private recommendation.
#[derive(Serialize, Deserialize)]
struct SealedBook {
    book_name: String,
    isbn: String,
}

#[derive(Serialize)]
pub struct RecommendationView {
    pub id: i64,
    pub book_name: String,
    pub isbn: String,
    pub is_public: bool,
    pub read: bool,
    pub recommendation_count: i64,
    pub timestamp: String,
}

#[derive(sqlx::FromRow)]
struct RecRow {
    id: i64,
    user_id: Option<i64>,
    book_name: Option<String>,
    isbn: Option<String>,
    enc_payload: Option<Vec<u8>>,
    is_public: bool,
    read: bool,
    recommendation_count: i64,
    timestamp: String,
}

const REC_COLUMNS: &str = "id, user_id, book_name, isbn, enc_payload, is_public, \
                           read, recommendation_count, timestamp";

/// Fetch recommendations for a page (or the global board when `user_id` is
/// `None`). With a private key, sealed rows are decrypted for the owner;
/// without one only public rows are returned.
pub async fn load_recommendations(
    pool: &SqlitePool,
    user_id: Option<i64>,
    private_key: Option<&SecretKey>,
) -> Result<Vec<RecommendationView>, AppError> {
    let sql = format!(
        "SELECT {REC_COLUMNS} FROM recommendations
         WHERE coalesce(user_id, 0) = ? {}
         ORDER BY recommendation_count DESC, timestamp DESC LIMIT 500",
        if private_key.is_some() { "" } else { "AND is_public = 1" }
    );
    let rows = sqlx::query_as::<_, RecRow>(sqlx::AssertSqlSafe(sql))
        .bind(user_id.unwrap_or(0))
        .fetch_all(pool)
        .await?;

    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        match row_to_view(row, private_key) {
            Some(view) => views.push(view),
            None => tracing::warn!("skipping undecryptable recommendation row"),
        }
    }
    Ok(views)
}

fn row_to_view(row: RecRow, private_key: Option<&SecretKey>) -> Option<RecommendationView> {
    let (book_name, isbn) = match (&row.book_name, &row.isbn, &row.enc_payload) {
        (Some(name), Some(isbn), _) => (name.clone(), isbn.clone()),
        (_, _, Some(payload)) => {
            let opened = crypto::unseal(private_key?, payload)?;
            let sealed: SealedBook = serde_json::from_slice(&opened).ok()?;
            (sealed.book_name, sealed.isbn)
        }
        _ => return None,
    };
    Some(RecommendationView {
        id: row.id,
        book_name,
        isbn,
        is_public: row.is_public,
        read: row.read,
        recommendation_count: row.recommendation_count,
        timestamp: row.timestamp,
    })
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
}

/// Typeahead suggestions, proxied through the resolver so the browser never
/// talks to Open Library directly (the CSP forbids it anyway).
pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<crate::resolver::BookSuggestion>>, AppError> {
    let q = query.q.trim();
    if q.is_empty() || q.chars().count() > MAX_INPUT_LEN {
        return Ok(Json(Vec::new()));
    }
    let suggestions = state.resolver.search(q).await.map_err(|_| AppError::Upstream)?;
    Ok(Json(suggestions))
}

pub async fn get_global(
    State(state): State<AppState>,
) -> Result<Json<Vec<RecommendationView>>, AppError> {
    Ok(Json(load_recommendations(&state.pool, None, None).await?))
}

#[derive(Deserialize)]
pub struct SubmitBody {
    pub book_name: String,
}

pub async fn submit_to_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    jar: CookieJar,
    Json(body): Json<SubmitBody>,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    submit(state, Some(user), jar, body).await
}

pub async fn submit_global(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<SubmitBody>,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    submit(state, None, jar, body).await
}

async fn submit(
    state: AppState,
    target: Option<User>,
    jar: CookieJar,
    body: SubmitBody,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    let input = body.book_name.trim();
    if input.is_empty() {
        return Err(AppError::BadRequest("enter a book name or ISBN".into()));
    }
    if input.chars().count() > MAX_INPUT_LEN {
        return Err(AppError::BadRequest("book name is too long".into()));
    }

    // Cross-reference against Open Library: only real, ISBN-verified books
    // are ever stored. This is the anti-spam / anti-garbage gate.
    let resolved = match isbn::looks_like_isbn(input) {
        Some(cleaned) => {
            let isbn13 = isbn::normalize(&cleaned)
                .ok_or_else(|| AppError::BadRequest("that ISBN has an invalid checksum".into()))?;
            state
                .resolver
                .by_isbn(&isbn13)
                .await
                .map_err(|_| AppError::Upstream)?
                .ok_or_else(|| AppError::BadRequest("no book found for that ISBN".into()))?
        }
        None => state
            .resolver
            .by_title(input)
            .await
            .map_err(|_| AppError::Upstream)?
            .ok_or_else(|| {
                AppError::BadRequest("no matching book found — try the exact title or an ISBN".into())
            })?,
    };
    let mut title = resolved.title;
    title.truncate(title.chars().take(MAX_INPUT_LEN).map(char::len_utf8).sum());

    let (submitter, jar) = auth::submitter_hash(jar, state.config.secure_cookies);
    let user_id = target.as_ref().map(|u| u.id);
    let blind = crypto::blind_index(&state.config.pepper, user_id, &resolved.isbn13);

    // Two attempts: a concurrent first-insert of the same book loses the
    // unique-index race and is retried as an increment.
    for attempt in 0..2 {
        let mut tx = state.pool.begin().await?;
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM recommendations WHERE isbn_index = ?")
                .bind(&blind)
                .fetch_optional(&mut *tx)
                .await?;

        if let Some((rec_id,)) = existing {
            let inserted = sqlx::query(
                "INSERT OR IGNORE INTO recommendation_votes (recommendation_id, submitter_hash)
                 VALUES (?, ?)",
            )
            .bind(rec_id)
            .bind(&submitter)
            .execute(&mut *tx)
            .await?
            .rows_affected();

            let status = if inserted == 1 {
                sqlx::query(
                    "UPDATE recommendations
                     SET recommendation_count = recommendation_count + 1 WHERE id = ?",
                )
                .bind(rec_id)
                .execute(&mut *tx)
                .await?;
                "counted"
            } else {
                "already_recommended"
            };
            tx.commit().await?;
            return Ok(respond(jar, status, &title, &resolved.isbn13));
        }

        let is_public = target.as_ref().map(|u| u.default_public).unwrap_or(true);
        let result = if is_public {
            sqlx::query(
                "INSERT INTO recommendations (user_id, book_name, isbn, isbn_index, user_key, is_public)
                 VALUES (?, ?, ?, ?, ?, 1)",
            )
            .bind(user_id)
            .bind(&title)
            .bind(&resolved.isbn13)
            .bind(&blind)
            .bind(&submitter)
            .execute(&mut *tx)
            .await
        } else {
            let target = target.as_ref().expect("private submissions always have a target page");
            let payload = serde_json::to_vec(&SealedBook {
                book_name: title.clone(),
                isbn: resolved.isbn13.clone(),
            })
            .expect("sealed payload serializes");
            let sealed = crypto::seal(&target.public_key, &payload)?;
            sqlx::query(
                "INSERT INTO recommendations (user_id, enc_payload, isbn_index, user_key, is_public)
                 VALUES (?, ?, ?, ?, 0)",
            )
            .bind(user_id)
            .bind(&sealed)
            .bind(&blind)
            .bind(&submitter)
            .execute(&mut *tx)
            .await
        };

        match result {
            Ok(done) => {
                let rec_id = done.last_insert_rowid();
                sqlx::query(
                    "INSERT INTO recommendation_votes (recommendation_id, submitter_hash)
                     VALUES (?, ?)",
                )
                .bind(rec_id)
                .bind(&submitter)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                return Ok(respond(jar, "created", &title, &resolved.isbn13));
            }
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() && attempt == 0 => {
                continue;
            }
            Err(err) => return Err(err.into()),
        }
    }
    unreachable!("second attempt either increments or errors")
}

fn respond(
    jar: CookieJar,
    status: &str,
    title: &str,
    isbn13: &str,
) -> (CookieJar, Json<serde_json::Value>) {
    (jar, Json(json!({ "status": status, "book": { "title": title, "isbn": isbn13 } })))
}

async fn owned_rec(
    state: &AppState,
    rec_id: i64,
    headers: &HeaderMap,
) -> Result<(RecRow, User, String), AppError> {
    let sql = format!("SELECT {REC_COLUMNS} FROM recommendations WHERE id = ?");
    let row = sqlx::query_as::<_, RecRow>(sqlx::AssertSqlSafe(sql))
        .bind(rec_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    // Global-board rows have no owner and cannot be managed.
    let user_id = row.user_id.ok_or(AppError::NotFound)?;
    let user = auth::user_by_id(&state.pool, user_id).await?.ok_or(AppError::NotFound)?;
    let secret = auth::require_owner(&user, headers)?;
    Ok((row, user, secret))
}

#[derive(Deserialize)]
pub struct RecPatch {
    pub read: Option<bool>,
    pub is_public: Option<bool>,
}

pub async fn patch_rec(
    State(state): State<AppState>,
    Path(rec_id): Path<i64>,
    headers: HeaderMap,
    Json(body): Json<RecPatch>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (row, user, secret) = owned_rec(&state, rec_id, &headers).await?;

    if let Some(read) = body.read {
        sqlx::query("UPDATE recommendations SET read = ? WHERE id = ?")
            .bind(read)
            .bind(rec_id)
            .execute(&state.pool)
            .await?;
    }

    if let Some(make_public) = body.is_public {
        if make_public && !row.is_public {
            // Reveal: decrypt with the owner's (bearer-derived) private key
            // and store plaintext.
            let private_key = crypto::unwrap_private_key(&secret, &user.enc_private_key)
                .ok_or_else(|| anyhow::anyhow!("stored private key failed to unwrap"))?;
            let payload = row.enc_payload.as_deref().ok_or_else(|| {
                anyhow::anyhow!("private recommendation is missing its sealed payload")
            })?;
            let opened = crypto::unseal(&private_key, payload)
                .ok_or_else(|| anyhow::anyhow!("sealed payload failed to open"))?;
            let sealed: SealedBook = serde_json::from_slice(&opened)
                .map_err(|e| anyhow::anyhow!("sealed payload is corrupt: {e}"))?;
            sqlx::query(
                "UPDATE recommendations
                 SET book_name = ?, isbn = ?, enc_payload = NULL, is_public = 1 WHERE id = ?",
            )
            .bind(&sealed.book_name)
            .bind(&sealed.isbn)
            .bind(rec_id)
            .execute(&state.pool)
            .await?;
        } else if !make_public && row.is_public {
            // Hide: seal the plaintext to the page's public key.
            let payload = serde_json::to_vec(&SealedBook {
                book_name: row.book_name.clone().unwrap_or_default(),
                isbn: row.isbn.clone().unwrap_or_default(),
            })
            .expect("sealed payload serializes");
            let sealed = crypto::seal(&user.public_key, &payload)?;
            sqlx::query(
                "UPDATE recommendations
                 SET book_name = NULL, isbn = NULL, enc_payload = ?, is_public = 0 WHERE id = ?",
            )
            .bind(&sealed)
            .bind(rec_id)
            .execute(&state.pool)
            .await?;
        }
    }

    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_rec(
    State(state): State<AppState>,
    Path(rec_id): Path<i64>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    owned_rec(&state, rec_id, &headers).await?;
    sqlx::query("DELETE FROM recommendations WHERE id = ?")
        .bind(rec_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}
