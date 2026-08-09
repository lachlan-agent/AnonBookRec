//! Owner bearer auth and anonymous submitter identity.

use axum::http::HeaderMap;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use sqlx::SqlitePool;

use crate::crypto;
use crate::error::AppError;

pub const ANON_COOKIE: &str = "anon_key";

#[derive(Debug, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub slug: String,
    pub secret_hash: String,
    pub public_key: Vec<u8>,
    pub enc_private_key: Vec<u8>,
    pub default_public: bool,
    pub display_name: Option<String>,
}

pub async fn user_by_slug(pool: &SqlitePool, slug: &str) -> Result<Option<User>, AppError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, slug, secret_hash, public_key, enc_private_key, default_public, display_name
         FROM users WHERE slug = ?",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

pub async fn user_by_id(pool: &SqlitePool, id: i64) -> Result<Option<User>, AppError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, slug, secret_hash, public_key, enc_private_key, default_public, display_name
         FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

pub fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(|s| s.trim().to_string())
}

/// The presented secret if it matches this user, else `Unauthorized`.
pub fn require_owner(user: &User, headers: &HeaderMap) -> Result<String, AppError> {
    let secret = bearer(headers).ok_or(AppError::Unauthorized)?;
    if crypto::verify_secret(&secret, &user.secret_hash) {
        Ok(secret)
    } else {
        Err(AppError::Unauthorized)
    }
}

/// Hash of the submitter's anonymous key, minting a new HttpOnly cookie on
/// first contact. Only the hash ever touches the database.
pub fn submitter_hash(jar: CookieJar, secure: bool) -> (String, CookieJar) {
    if let Some(cookie) = jar.get(ANON_COOKIE) {
        let key = cookie.value().to_string();
        return (crypto::sha256_hex(key.as_bytes()), jar);
    }
    let key = crypto::generate_secret();
    let hash = crypto::sha256_hex(key.as_bytes());
    let cookie = Cookie::build((ANON_COOKIE, key))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .max_age(time::Duration::days(365))
        .build();
    (hash, jar.add(cookie))
}
