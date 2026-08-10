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
pub struct DraftPage {
    pub slug: String,
    pub secret: String,
    pub url: String,
    /// HMAC over (slug, secret) proving this draft was issued by us. Sent
    /// back on confirm; nothing is stored server-side in the meantime.
    pub token: String,
}

/// Stage a page without touching the database. The client shows the secret,
/// makes the person confirm they saved it, and only then confirms creation
/// via [`create_page`]. An abandoned draft leaves no trace.
pub async fn draft_page(State(state): State<AppState>) -> Json<DraftPage> {
    let secret = crypto::generate_secret();
    let slug = crypto::generate_slug();
    let token = crypto::draft_token(&state.config.pepper, &slug, &secret);
    let url = format!("{}/u/{}", state.config.public_base_url, slug);
    Json(DraftPage { slug, secret, url, token })
}

#[derive(Deserialize)]
pub struct CreatePageBody {
    pub slug: String,
    pub secret: String,
    pub token: String,
}

/// Create the page a draft described, once the owner has confirmed saving
/// the secret. The token gate means only server-issued (slug, secret) pairs
/// are accepted; only the secret's hash is stored.
pub async fn create_page(
    State(state): State<AppState>,
    Json(body): Json<CreatePageBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !crypto::verify_draft_token(&state.config.pepper, &body.slug, &body.secret, &body.token) {
        return Err(AppError::Unauthorized);
    }
    let secret_hash = crypto::hash_secret(&body.secret);
    let (public_key, enc_private_key) = crypto::new_page_keys(&body.secret);

    let result = sqlx::query(
        "INSERT INTO users (slug, secret_hash, public_key, enc_private_key) VALUES (?, ?, ?, ?)",
    )
    .bind(&body.slug)
    .bind(&secret_hash)
    .bind(&public_key)
    .bind(&enc_private_key)
    .execute(&state.pool)
    .await;
    match result {
        Ok(_) => {
            let url = format!("{}/u/{}", state.config.public_base_url, body.slug);
            Ok(Json(json!({ "slug": body.slug, "url": url })))
        }
        // Replaying a confirmed draft (double-click, retry) or the
        // astronomically unlikely slug collision.
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(AppError::Conflict("that page already exists — request a new one".into()))
        }
        Err(err) => Err(err.into()),
    }
}

/// Hidden pages exist only behind their share token: any other identifier
/// (or a missing/wrong bearer) gets a plain 404, so the page is
/// indistinguishable from one that never existed.
pub fn require_visible(
    user: &crate::auth::User,
    requested: &str,
    headers: &HeaderMap,
) -> Result<(), AppError> {
    if !user.is_hidden || user.share_token.as_deref() == Some(requested) {
        return Ok(());
    }
    if let Some(secret) = auth::bearer(headers)
        && crypto::verify_secret(&secret, &user.secret_hash)
    {
        return Ok(());
    }
    Err(AppError::NotFound)
}

#[derive(Serialize)]
pub struct PageView {
    pub slug: String,
    /// Public handle for URLs: username when set, else the slug.
    pub handle: String,
    pub username: Option<String>,
    pub is_hidden: bool,
    /// Friend link token — owner's eyes only.
    pub share_token: Option<String>,
    pub owner: bool,
    pub default_public: bool,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub favourite_book: Option<String>,
    pub theme: Option<String>,
    pub font: Option<String>,
    pub accent: Option<String>,
    pub radius: Option<String>,
    pub density: Option<String>,
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
    require_visible(&user, &slug, &headers)?;

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
        is_hidden: user.is_hidden,
        share_token: if owner { user.share_token.clone() } else { None },
        slug: user.slug,
        owner,
        default_public: user.default_public,
        display_name: user.display_name,
        bio: show_profile.then_some(user.bio).flatten(),
        favourite_book: show_profile.then_some(user.favourite_book).flatten(),
        theme: show_profile.then_some(user.theme).flatten(),
        font: show_profile.then_some(user.font).flatten(),
        accent: show_profile.then_some(user.accent).flatten(),
        radius: show_profile.then_some(user.radius).flatten(),
        density: show_profile.then_some(user.density).flatten(),
        recommendations,
    }))
}

pub const THEMES: &[&str] = &[
    "cloth", "ink", "plum", "amber", "night", "midnight", "noir", "garnet", "forest", "cosmos",
    "prism", "manuscript", "blueprint", "ocean", "ember", "sakura", "aurora", "rain", "fireflies",
    "snow", "constellation", "matrix", "koi", "lava", "flock", "flock-glass",
];

#[derive(Deserialize)]
pub struct PagePatch {
    pub default_public: Option<bool>,
    pub is_hidden: Option<bool>,
    /// Optional public name for the page. Empty string clears it. Never part
    /// of the URL — the slug stays random.
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub favourite_book: Option<String>,
    pub theme: Option<String>,
    pub font: Option<String>,
    pub accent: Option<String>,
    pub radius: Option<String>,
    pub density: Option<String>,
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

    if let Some(is_hidden) = body.is_hidden {
        // Hiding does NOT mint a share link: a freshly hidden page is
        // reachable by its owner alone until a friend link is requested.
        sqlx::query("UPDATE users SET is_hidden = ? WHERE id = ?")
            .bind(is_hidden)
            .bind(user.id)
            .execute(&state.pool)
            .await?;
    }

    let updates = [
        ("display_name", clean_text(body.display_name, 64, "display name")?),
        ("bio", clean_text(body.bio, 500, "bio")?),
        ("favourite_book", clean_text(body.favourite_book, 256, "favourite book")?),
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

    // Presentation options live in user_theme (upserted); each is a
    // whitelisted enum or a strict hex colour — raw CSS never enters.
    let theme_updates = [
        ("theme", clean_enum(body.theme, THEMES, "theme")?),
        ("font", clean_enum(body.font, FONTS, "font")?),
        ("radius", clean_enum(body.radius, RADII, "corner style")?),
        ("density", clean_enum(body.density, DENSITIES, "density")?),
        ("accent", clean_accent(body.accent)?),
    ];
    for (column, value) in theme_updates {
        if let Some(value) = value {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO user_theme (user_id, {column}) VALUES (?1, ?2)
                 ON CONFLICT(user_id) DO UPDATE SET {column} = excluded.{column}"
            )))
            .bind(user.id)
            .bind(value)
            .execute(&state.pool)
            .await?;
        }
    }

    Ok(Json(json!({ "ok": true })))
}

pub const FONTS: &[&str] = &["classic", "modern", "typewriter", "elegant", "humanist"];
pub const RADII: &[&str] = &["rounded", "square", "pill"];
pub const DENSITIES: &[&str] = &["cozy", "compact"];

/// Whitelist-validated optional enum; empty clears.
fn clean_enum(
    field: Option<String>,
    allowed: &[&str],
    label: &str,
) -> Result<Option<Option<String>>, AppError> {
    let Some(cleaned) = clean_text(field, 24, label)? else { return Ok(None) };
    if let Some(value) = &cleaned
        && !allowed.contains(&value.as_str())
    {
        return Err(AppError::BadRequest(format!("unknown {label} (try {})", allowed.join(", "))));
    }
    Ok(Some(cleaned))
}

/// Accent colour: exactly `#rrggbb`, stored lowercase; empty clears.
fn clean_accent(field: Option<String>) -> Result<Option<Option<String>>, AppError> {
    let Some(cleaned) = clean_text(field, 7, "accent colour")? else { return Ok(None) };
    let Some(value) = cleaned else { return Ok(Some(None)) };
    let value = value.to_lowercase();
    let valid = value.len() == 7
        && value.starts_with('#')
        && value[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !valid {
        return Err(AppError::BadRequest("accent colour must be #rrggbb".into()));
    }
    Ok(Some(Some(value)))
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
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    require_visible(&user, &slug, &headers)?;
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

/// Mint (or rotate) the friend share link for a hidden page. Regenerating
/// silently revokes the previous link.
pub async fn create_share_link(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    auth::require_owner(&user, &headers)?;
    let token = crypto::generate_secret();
    sqlx::query("UPDATE users SET share_token = ? WHERE id = ?")
        .bind(&token)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    let url = format!("{}/u/{}", state.config.public_base_url, token);
    Ok(Json(json!({ "share_token": token, "url": url })))
}

/// Revoke the friend link. The response carries the canonical handle so the
/// owner's browser can leave the now-dead token URL.
pub async fn revoke_share_link(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    auth::require_owner(&user, &headers)?;
    sqlx::query("UPDATE users SET share_token = NULL WHERE id = ?")
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    let canonical = user.username.as_deref().unwrap_or(&user.slug);
    Ok(Json(json!({ "ok": true, "handle": canonical })))
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
        "SELECT u.id, u.slug, u.secret_hash, u.public_key, u.enc_private_key, \
         u.default_public, u.display_name, u.bio, u.favourite_book, u.username, \
         u.password_hash, u.enc_secret, u.is_hidden, u.share_token, \
         t.theme, t.font, t.accent, t.radius, t.density \
         FROM users u LEFT JOIN user_theme t ON t.user_id = u.id WHERE u.username = ?"
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

pub async fn list_bans(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    auth::require_owner(&user, &headers)?;
    let bans: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT isbn, title FROM user_book_blacklist WHERE user_id = ? ORDER BY rowid DESC",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    let bans: Vec<_> = bans
        .into_iter()
        .map(|(isbn, title)| json!({ "isbn": isbn, "title": title }))
        .collect();
    Ok(Json(json!({ "bans": bans })))
}

pub async fn unban(
    State(state): State<AppState>,
    Path((slug, isbn)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    auth::require_owner(&user, &headers)?;
    sqlx::query("DELETE FROM user_book_blacklist WHERE user_id = ? AND isbn = ?")
        .bind(user.id)
        .bind(&isbn)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// RSS 2.0 feed of a page's public recommendations. Follows the same
/// visibility rules as the page itself (hidden pages: share-token URL only).
pub async fn feed(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let user = auth::user_by_slug(&state.pool, &slug).await?.ok_or(AppError::NotFound)?;
    require_visible(&user, &slug, &headers)?;

    let rows: Vec<(i64, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, book_name, author, isbn, timestamp FROM recommendations
         WHERE user_id = ? AND is_public = 1
         ORDER BY timestamp DESC, id DESC LIMIT 50",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;

    let page_url = format!("{}/u/{}", state.config.public_base_url, slug);
    let page_title = match user.display_name.as_deref() {
        Some(name) => format!("{name}'s book recommendations"),
        None => "Book recommendations".to_string(),
    };

    let format = time::format_description::well_known::Rfc2822;
    let sqlite_format = time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second]"
    );
    let mut items = String::new();
    for (id, title, author, isbn, timestamp) in rows {
        let pub_date = time::PrimitiveDateTime::parse(&timestamp, &sqlite_format)
            .ok()
            .and_then(|dt| dt.assume_utc().format(&format).ok())
            .unwrap_or_default();
        let item_title = match &author {
            Some(author) => format!("{title} — {author}"),
            None => title.clone(),
        };
        items.push_str(&format!(
            "<item><title>{}</title><link>{}</link>\
             <guid isPermaLink=\"false\">rec-{id}-{}</guid>\
             <pubDate>{}</pubDate>\
             <description>ISBN {}</description></item>",
            escape_xml(&item_title),
            escape_xml(&page_url),
            escape_xml(&isbn),
            escape_xml(&pub_date),
            escape_xml(&isbn),
        ));
    }

    let rss = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <rss version=\"2.0\"><channel>\
         <title>{}</title><link>{}</link>\
         <description>Anonymous book recommendations</description>\
         {items}</channel></rss>",
        escape_xml(&page_title),
        escape_xml(&page_url),
    );
    Ok(([(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")], rss))
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
