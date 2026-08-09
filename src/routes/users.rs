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
    Ok(Json(PageView {
        slug: user.slug,
        owner: private_key.is_some(),
        default_public: user.default_public,
        display_name: user.display_name,
        recommendations,
    }))
}

#[derive(Deserialize)]
pub struct PagePatch {
    pub default_public: Option<bool>,
    /// Optional public name for the page. Empty string clears it. Never part
    /// of the URL — the slug stays random.
    pub display_name: Option<String>,
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

    let mut display_name = body.display_name;
    if let Some(name) = &mut display_name {
        let trimmed = name.trim();
        if trimmed.chars().count() > 64 {
            return Err(AppError::BadRequest("display name is too long (max 64)".into()));
        }
        let cleared = trimmed.is_empty();
        *name = trimmed.to_string();
        sqlx::query("UPDATE users SET display_name = ? WHERE id = ?")
            .bind(if cleared { None } else { Some(name.clone()) })
            .bind(user.id)
            .execute(&state.pool)
            .await?;
    }

    Ok(Json(json!({ "ok": true })))
}
