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
    pub bio: Option<String>,
    pub favourite_book: Option<String>,
    pub theme: Option<String>,
    pub username: Option<String>,
    pub password_hash: Option<String>,
    pub enc_secret: Option<Vec<u8>>,
    pub is_hidden: bool,
    pub share_token: Option<String>,
    pub font: Option<String>,
    pub accent: Option<String>,
    pub radius: Option<String>,
    pub density: Option<String>,
}

impl User {
    /// Public handle for URLs. Hidden pages hand out only their long share
    /// token; otherwise the chosen username, else the random slug.
    pub fn handle(&self) -> &str {
        if self.is_hidden
            && let Some(token) = self.share_token.as_deref()
        {
            return token;
        }
        self.username.as_deref().unwrap_or(&self.slug)
    }
}

const USER_COLUMNS: &str = "u.id, u.slug, u.secret_hash, u.public_key, u.enc_private_key, \
                            u.default_public, u.display_name, u.bio, u.favourite_book, \
                            u.username, u.password_hash, u.enc_secret, u.is_hidden, \
                            u.share_token, t.theme, t.font, t.accent, t.radius, t.density";

const USER_FROM: &str = "users u LEFT JOIN user_theme t ON t.user_id = u.id";

/// Look a page up by its handle: random slug or chosen username. Old slug
/// URLs keep working after a username is set.
pub async fn user_by_slug(pool: &SqlitePool, slug: &str) -> Result<Option<User>, AppError> {
    // Slugs and share tokens are case-sensitive base58; usernames lowercase.
    let user = sqlx::query_as::<_, User>(sqlx::AssertSqlSafe(format!(
        "SELECT {USER_COLUMNS} FROM {USER_FROM} WHERE u.slug = ?1 OR u.username = ?2 OR u.share_token = ?1"
    )))
    .bind(slug)
    .bind(slug.to_lowercase())
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

pub async fn user_by_id(pool: &SqlitePool, id: i64) -> Result<Option<User>, AppError> {
    let user = sqlx::query_as::<_, User>(sqlx::AssertSqlSafe(format!(
        "SELECT {USER_COLUMNS} FROM {USER_FROM} WHERE u.id = ?"
    )))
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
