use axum::extract::{Path, State};
use axum::http::HeaderMap;
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

pub const THEMES: &[&str] =
    &["cloth", "ink", "plum", "amber", "night", "midnight", "noir", "garnet"];

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
