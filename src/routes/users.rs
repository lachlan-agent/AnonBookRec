use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth;
use crate::crypto;
use crate::error::AppError;
use crate::routes::recommendations::{load_recommendations, RecommendationView};
use crate::state::AppState;

#[derive(Serialize)]
pub struct CreatedPage {
    pub slug: String,
    pub secret: String,
    pub url: String,
}

/// Create an anonymous page. The secret is returned exactly once and only its
/// hash is stored.
pub async fn create_page(State(state): State<AppState>) -> Result<Json<CreatedPage>, AppError> {
    let secret = crypto::generate_secret();
    let secret_hash = crypto::hash_secret(&secret);
    let (public_key, enc_private_key) = crypto::new_page_keys(&secret);

    // Retry on the (astronomically unlikely) slug collision.
    for _ in 0..3 {
        let slug = crypto::generate_slug();
        let result = sqlx::query(
            "INSERT INTO users (slug, secret_hash, public_key, enc_private_key) VALUES (?, ?, ?, ?)",
        )
        .bind(&slug)
        .bind(&secret_hash)
        .bind(&public_key)
        .bind(&enc_private_key)
        .execute(&state.pool)
        .await;
        match result {
            Ok(_) => {
                let url = format!("{}/u/{}", state.config.public_base_url, slug);
                return Ok(Json(CreatedPage { slug, secret, url }));
            }
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Err(AppError::Internal(anyhow::anyhow!("could not allocate a unique slug")))
}

#[derive(Serialize)]
pub struct PageView {
    pub slug: String,
    /// Public handle for URLs: username when set, else the slug.
    pub handle: String,
    pub username: Option<String>,
    pub owner: bool,
    pub default_public: bool,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub favourite_book: Option<String>,
    pub theme: Option<String>,
    pub recommendations: Vec<RecommendationView>,
}

/// Public view shows public recommendations only; the owner (bearer secret)
/// additionally sees private ones, decrypted in memory for this response.
pub async fn get_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Json<PageView>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;

    let private_key = match auth::bearer(&headers) {
        Some(_) => {
            let secret = auth::require_owner(&user, &headers)?;
            Some(
                crypto::unwrap_private_key(&secret, &user.enc_private_key)
                    .ok_or_else(|| anyhow::anyhow!("stored private key failed to unwrap"))?,
            )
        }
        None => None,
    };

    let recommendations =
        load_recommendations(&state.pool, Some(user.id), private_key.as_ref()).await?;
    // The profile (bio, favourite book, theme) belongs to public pages only:
    // a private page shows visitors nothing but the submission form.
    let owner = private_key.is_some();
    let show_profile = owner || user.default_public;
    Ok(Json(PageView {
        handle: user.handle().to_string(),
        username: user.username.clone(),
        slug: user.slug,
        owner,
        default_public: user.default_public,
        display_name: user.display_name,
        bio: show_profile.then_some(user.bio).flatten(),
        favourite_book: show_profile.then_some(user.favourite_book).flatten(),
        theme: show_profile.then_some(user.theme).flatten(),
        recommendations,
    }))
}

pub const THEMES: &[&str] = &[
    "cloth", "ink", "plum", "amber", "night", "midnight", "noir", "garnet", "forest", "cosmos",
    "prism", "manuscript", "blueprint", "ocean", "ember", "sakura", "aurora",
];

#[derive(Deserialize)]
pub struct PagePatch {
    pub default_public: Option<bool>,
    /// Optional public name for the page. Empty string clears it. Never part
    /// of the URL — the slug stays random.
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub favourite_book: Option<String>,
    pub theme: Option<String>,
}

/// Trim an optional text field; empty clears the column (`Some(None)`),
/// absent means "leave unchanged" (`None`).
fn clean_text(
    field: Option<String>,
    max: usize,
    label: &str,
) -> Result<Option<Option<String>>, AppError> {
    let Some(value) = field else { return Ok(None) };
    let trimmed = value.trim();
    if trimmed.chars().count() > max {
        return Err(AppError::BadRequest(format!("{label} is too long (max {max})")));
    }
    Ok(Some((!trimmed.is_empty()).then(|| trimmed.to_string())))
}

pub async fn patch_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PagePatch>,
) -> Result<Json<serde_json::Value>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    auth::require_owner(&user, &headers)?;

    if let Some(default_public) = body.default_public {
        sqlx::query("UPDATE users SET default_public = ? WHERE id = ?")
            .bind(default_public)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
    }

    let theme = clean_text(body.theme, 16, "theme")?;
    if let Some(Some(theme)) = &theme
        && !THEMES.contains(&theme.as_str())
    {
        return Err(AppError::BadRequest(format!("unknown theme (try {})", THEMES.join(", "))));
    }

    let updates = [
        ("display_name", clean_text(body.display_name, 64, "display name")?),
        ("bio", clean_text(body.bio, 500, "bio")?),
        ("favourite_book", clean_text(body.favourite_book, 256, "favourite book")?),
        ("theme", theme),
    ];
    for (column, value) in updates {
        if let Some(value) = value {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE users SET {column} = ? WHERE id = ?"
            )))
            .bind(value)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        }
    }

    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize)]
pub struct RotatedKey {
    pub secret: String,
}

/// Replace the owner's secret with a freshly generated one (owner only — the
/// current secret must be presented as the bearer). The page keypair is
/// preserved: the private key is unwrapped with the old secret and re-wrapped
/// under the new one, so sealed recommendations stay readable. The old secret
/// stops working the moment the update lands.
pub async fn rotate_key(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RotatedKey>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    let old_secret = auth::require_owner(&user, &headers)?;
    let private = crypto::unwrap_private_key(&old_secret, &user.enc_private_key)
        .ok_or_else(|| anyhow::anyhow!("stored private key failed to unwrap"))?;

    let new_secret = crypto::generate_secret();
    let new_hash = crypto::hash_secret(&new_secret);
    let rewrapped = crypto::wrap_private_key(&new_secret, &private);

    // Guard on the old hash so concurrent rotations can't both win; the loser
    // matched 0 rows, meaning the key it presented is already stale.
    let updated = sqlx::query(
        // enc_secret is the old secret under the password: stale after
        // rotation, so password login is disabled until credentials are re-set.
        "UPDATE users SET secret_hash = ?, enc_private_key = ?, enc_secret = NULL
         WHERE id = ? AND secret_hash = ?",
    )
    .bind(&new_hash)
    .bind(&rewrapped)
    .bind(user.id)
    .bind(&user.secret_hash)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(AppError::Unauthorized);
    }
    Ok(Json(RotatedKey { secret: new_secret }))
}

/// SVG QR code of the page's share URL. No auth: the slug in the request path
/// already is the share URL's secret-free public half.
pub async fn qr_svg(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    let url = format!("{}/u/{}", state.config.public_base_url, user.handle());
    let code = qrcode::QrCode::new(url.as_bytes())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("qr encoding failed: {e}")))?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    Ok((
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        svg,
    ))
}

#[derive(Deserialize)]
pub struct CredentialsBody {
    pub username: String,
    pub password: String,
}

fn validate_username(raw: &str) -> Result<String, AppError> {
    let username = raw.trim().to_lowercase();
    let ok_chars = username
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if username.len() < 3 || username.len() > 32 || !ok_chars {
        return Err(AppError::BadRequest(
            "username must be 3–32 characters: a–z, 0–9, _ or -".into(),
        ));
    }
    Ok(username)
}

/// Attach (or replace) a username + password. The password becomes a second
/// way to recover the page secret; the secret key remains the backup.
pub async fn set_credentials(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CredentialsBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    let secret = auth::require_owner(&user, &headers)?;

    let username = validate_username(&body.username)?;
    let password_len = body.password.chars().count();
    if !(8..=128).contains(&password_len) {
        return Err(AppError::BadRequest("password must be 8–128 characters".into()));
    }

    // Usernames share the /u/{…} namespace with slugs: collide with neither.
    let taken: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM users WHERE (username = ?1 OR slug = ?1) AND id != ?2")
            .bind(&username)
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await?;
    if taken.is_some() {
        return Err(AppError::Conflict("that username is taken".into()));
    }

    let password_hash = crypto::hash_password(&body.password)?;
    let enc_secret = crypto::wrap_secret_with_password(&body.password, &secret);
    let result = sqlx::query(
        "UPDATE users SET username = ?, password_hash = ?, enc_secret = ? WHERE id = ?",
    )
    .bind(&username)
    .bind(&password_hash)
    .bind(&enc_secret)
    .bind(user.id)
    .execute(&state.pool)
    .await;
    match result {
        Ok(_) => Ok(Json(json!({ "username": username, "handle": username }))),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(AppError::Conflict("that username is taken".into()))
        }
        Err(err) => Err(err.into()),
    }
}

/// Remove password login. The username (and its URL) survives so shared links
/// keep working; only the password path is disabled.
pub async fn delete_credentials(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    auth::require_owner(&user, &headers)?;
    sqlx::query("UPDATE users SET password_hash = NULL, enc_secret = NULL WHERE id = ?")
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
}

/// Password login: verify, decrypt the stored secret, hand it to the client.
/// Uniform 401 on every failure mode — no username enumeration.
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let username = body.username.trim().to_lowercase();
    let user = sqlx::query_as::<_, auth::User>(sqlx::AssertSqlSafe(
        "SELECT id, slug, secret_hash, public_key, enc_private_key, default_public, \
         display_name, bio, favourite_book, theme, username, password_hash, enc_secret \
         FROM users WHERE username = ?"
            .to_string(),
    ))
    .bind(&username)
    .fetch_optional(&state.pool)
    .await?;

    let Some(user) = user else { return Err(AppError::Unauthorized) };
    let (Some(phc), Some(enc_secret)) = (&user.password_hash, &user.enc_secret) else {
        return Err(AppError::Unauthorized);
    };
    if !crypto::verify_password(&body.password, phc) {
        return Err(AppError::Unauthorized);
    }
    let secret = crypto::unwrap_secret_with_password(&body.password, enc_secret)
        .ok_or(AppError::Unauthorized)?;
    Ok(Json(json!({ "handle": user.handle(), "secret": secret })))
}

/// Permanently delete the page and everything attached to it (owner only).
/// Recommendations and votes go with it via ON DELETE CASCADE.
pub async fn delete_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    auth::require_owner(&user, &headers)?;
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}
