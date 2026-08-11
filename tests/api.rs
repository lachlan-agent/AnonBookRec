//! End-to-end API tests against an in-memory SQLite database and a stubbed
//! book resolver (no network).

use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use anonbookrec::app::build_app;
use anonbookrec::config::Config;
use anonbookrec::db::MIGRATOR;
use anonbookrec::resolver::{BookResolver, BookSuggestion, ResolvedBook, ResolverError};
use anonbookrec::state::AppState;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tower::ServiceExt;

const KNOWN_ISBN: &str = "9780134685991";
const KNOWN_ISBN_TITLE: &str = "Effective Java";
const TITLE_MATCH_ISBN: &str = "9780306406157";

struct StubResolver;

#[async_trait::async_trait]
impl BookResolver for StubResolver {
    async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        Ok((isbn13 == KNOWN_ISBN).then(|| ResolvedBook {
            title: KNOWN_ISBN_TITLE.to_string(),
            isbn13: isbn13.to_string(),
            author: Some("Joshua Bloch".to_string()),
        }))
    }

    async fn by_title(&self, title: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        Ok(title.to_lowercase().contains("dispossessed").then(|| ResolvedBook {
            title: "The Dispossessed".to_string(),
            isbn13: TITLE_MATCH_ISBN.to_string(),
            author: Some("Ursula K. Le Guin".to_string()),
        }))
    }

    async fn search(&self, query: &str) -> Result<Vec<BookSuggestion>, ResolverError> {
        Ok(query
            .to_lowercase()
            .contains("dispossessed")
            .then(|| BookSuggestion {
                title: "The Dispossessed".to_string(),
                author: Some("Ursula K. Le Guin".to_string()),
                isbn13: TITLE_MATCH_ISBN.to_string(),
                year: Some(1974),
                cover_id: Some(12345),
            })
            .into_iter()
            .collect())
    }
}

async fn test_app() -> (Router, SqlitePool) {
    let options = SqliteConnectOptions::from_str("sqlite::memory:").unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    MIGRATOR.run(&pool).await.unwrap();

    let config = Config {
        bind: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        database_url: "sqlite::memory:".into(),
        public_base_url: "http://test.local".into(),
        secure_cookies: false,
        pepper: [42u8; 32],
        rate_limiting: false,
        static_dir: "static".into(),
        book_db: None,
    };
    let state = AppState {
        pool: pool.clone(),
        resolver: Arc::new(StubResolver),
        config: Arc::new(config),
    };
    (build_app(state), pool)
}

struct TestResponse {
    status: StatusCode,
    body: Value,
    cookie: Option<String>,
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    cookie: Option<&str>,
) -> TestResponse {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    let request = match body {
        Some(value) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => request.body(Body::empty()).unwrap(),
    };

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap().to_string());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    TestResponse { status, body, cookie }
}

/// Full two-phase creation: draft, then confirm (as the frontend does after
/// "I saved my key").
async fn create_page(app: &Router) -> (String, String) {
    let draft = call(app, "POST", "/api/users/draft", None, None, None).await;
    assert_eq!(draft.status, StatusCode::OK);
    let slug = draft.body["slug"].as_str().unwrap().to_string();
    let secret = draft.body["secret"].as_str().unwrap().to_string();

    let confirmed = call(app, "POST", "/api/users",
        Some(json!({"slug": slug, "secret": secret, "token": draft.body["token"]})),
        None, None).await;
    assert_eq!(confirmed.status, StatusCode::OK);
    (slug, secret)
}

#[tokio::test]
async fn draft_creates_nothing_until_confirmed() {
    let (app, pool) = test_app().await;

    // Draft alone (dialog dismissed): no row.
    let draft = call(&app, "POST", "/api/users/draft", None, None, None).await;
    assert_eq!(draft.status, StatusCode::OK);
    let slug = draft.body["slug"].as_str().unwrap().to_string();
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0, "a draft must not create a page");
    let missing = call(&app, "GET", &format!("/api/pages/{slug}"), None, None, None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    // Forged or tampered confirmation is rejected.
    let forged = call(&app, "POST", "/api/users",
        Some(json!({"slug": slug, "secret": draft.body["secret"], "token": "not-a-token"})),
        None, None).await;
    assert_eq!(forged.status, StatusCode::UNAUTHORIZED);
    let swapped = call(&app, "POST", "/api/users",
        Some(json!({"slug": "myvanityslug", "secret": draft.body["secret"],
                    "token": draft.body["token"]})),
        None, None).await;
    assert_eq!(swapped.status, StatusCode::UNAUTHORIZED, "token must bind the slug");

    // Real confirmation creates the page; replaying it conflicts.
    let confirmed = call(&app, "POST", "/api/users",
        Some(json!({"slug": slug, "secret": draft.body["secret"], "token": draft.body["token"]})),
        None, None).await;
    assert_eq!(confirmed.status, StatusCode::OK);
    let exists = call(&app, "GET", &format!("/api/pages/{slug}"), None, None, None).await;
    assert_eq!(exists.status, StatusCode::OK);
    let replay = call(&app, "POST", "/api/users",
        Some(json!({"slug": slug, "secret": draft.body["secret"], "token": draft.body["token"]})),
        None, None).await;
    assert_eq!(replay.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn page_creation_and_owner_auth() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;

    let owner = call(&app, "GET", &format!("/api/pages/{slug}"), None, Some(&secret), None).await;
    assert_eq!(owner.status, StatusCode::OK);
    assert_eq!(owner.body["owner"], json!(true));
    assert_eq!(owner.body["default_public"], json!(true));

    let visitor = call(&app, "GET", &format!("/api/pages/{slug}"), None, None, None).await;
    assert_eq!(visitor.body["owner"], json!(false));

    let wrong = call(&app, "GET", &format!("/api/pages/{slug}"), None, Some("bogus"), None).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

    let missing = call(&app, "GET", "/api/pages/doesnotexist", None, None, None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn credentials_login_and_username_urls() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let creds_url = format!("/api/pages/{slug}/credentials");

    // Validation and auth.
    let no_auth = call(&app, "POST", &creds_url,
        Some(json!({"username": "lachy", "password": "longenough"})), None, None).await;
    assert_eq!(no_auth.status, StatusCode::UNAUTHORIZED);
    let bad_name = call(&app, "POST", &creds_url,
        Some(json!({"username": "x", "password": "longenough"})), Some(&secret), None).await;
    assert_eq!(bad_name.status, StatusCode::BAD_REQUEST);
    let short_pw = call(&app, "POST", &creds_url,
        Some(json!({"username": "lachy", "password": "short"})), Some(&secret), None).await;
    assert_eq!(short_pw.status, StatusCode::BAD_REQUEST);

    // Set credentials (username normalized to lowercase).
    let set = call(&app, "POST", &creds_url,
        Some(json!({"username": "  Lachy  ", "password": "longenough"})), Some(&secret), None).await;
    assert_eq!(set.status, StatusCode::OK);
    assert_eq!(set.body["handle"], json!("lachy"));

    // Username is now the handle: API + page work by username AND old slug.
    let by_username = call(&app, "GET", "/api/pages/lachy", None, None, None).await;
    assert_eq!(by_username.status, StatusCode::OK);
    assert_eq!(by_username.body["handle"], json!("lachy"));
    let by_slug = call(&app, "GET", &format!("/api/pages/{slug}"), None, None, None).await;
    assert_eq!(by_slug.status, StatusCode::OK);
    let page = call(&app, "GET", "/u/lachy", None, None, None).await;
    assert_eq!(page.status, StatusCode::OK);

    // Login returns the same secret; wrong creds get a uniform 401.
    let login = call(&app, "POST", "/api/login",
        Some(json!({"username": "Lachy", "password": "longenough"})), None, None).await;
    assert_eq!(login.status, StatusCode::OK);
    assert_eq!(login.body["secret"], json!(secret));
    assert_eq!(login.body["handle"], json!("lachy"));
    let wrong_pw = call(&app, "POST", "/api/login",
        Some(json!({"username": "lachy", "password": "wrongwrong"})), None, None).await;
    assert_eq!(wrong_pw.status, StatusCode::UNAUTHORIZED);
    let no_user = call(&app, "POST", "/api/login",
        Some(json!({"username": "ghost", "password": "wrongwrong"})), None, None).await;
    assert_eq!(no_user.status, StatusCode::UNAUTHORIZED);

    // Taken by another page's username or slug → 409.
    let (slug2, secret2) = create_page(&app).await;
    let taken = call(&app, "POST", &format!("/api/pages/{slug2}/credentials"),
        Some(json!({"username": "lachy", "password": "longenough"})), Some(&secret2), None).await;
    assert_eq!(taken.status, StatusCode::CONFLICT);
    let slug_clash = call(&app, "POST", &format!("/api/pages/{slug2}/credentials"),
        Some(json!({"username": slug.to_lowercase(), "password": "longenough"})),
        Some(&secret2), None).await;
    // Only conflicts when the lowercased slug happens to match the real slug.
    if slug == slug.to_lowercase() {
        assert_eq!(slug_clash.status, StatusCode::CONFLICT);
    }

    // Removing credentials keeps the username URL but kills password login.
    let removed = call(&app, "DELETE", &creds_url, None, Some(&secret), None).await;
    assert_eq!(removed.status, StatusCode::OK);
    let login = call(&app, "POST", "/api/login",
        Some(json!({"username": "lachy", "password": "longenough"})), None, None).await;
    assert_eq!(login.status, StatusCode::UNAUTHORIZED);
    let page = call(&app, "GET", "/api/pages/lachy", None, None, None).await;
    assert_eq!(page.status, StatusCode::OK, "username URL survives");
}

#[tokio::test]
async fn rotation_disables_password_login_until_reset() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let creds_url = format!("/api/pages/{slug}/credentials");

    call(&app, "POST", &creds_url,
        Some(json!({"username": "rotator", "password": "longenough"})), Some(&secret), None).await;

    let rotated = call(&app, "POST", &format!("/api/pages/{slug}/rotate-key"),
        None, Some(&secret), None).await;
    assert_eq!(rotated.status, StatusCode::OK);
    let new_secret = rotated.body["secret"].as_str().unwrap().to_string();

    // Old enc_secret is stale → login refused until credentials are re-set.
    let login = call(&app, "POST", "/api/login",
        Some(json!({"username": "rotator", "password": "longenough"})), None, None).await;
    assert_eq!(login.status, StatusCode::UNAUTHORIZED);

    let reset = call(&app, "POST", &creds_url,
        Some(json!({"username": "rotator", "password": "longenough"})),
        Some(&new_secret), None).await;
    assert_eq!(reset.status, StatusCode::OK);
    let login = call(&app, "POST", "/api/login",
        Some(json!({"username": "rotator", "password": "longenough"})), None, None).await;
    assert_eq!(login.status, StatusCode::OK);
    assert_eq!(login.body["secret"], json!(new_secret));
}

#[tokio::test]
async fn hidden_pages_truly_hidden_until_friend_link_minted() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let page_url = format!("/api/pages/{slug}");

    call(&app, "POST", &format!("/api/pages/{slug}/credentials"),
        Some(json!({"username": "ghostwriter", "password": "longenough"})),
        Some(&secret), None).await;
    call(&app, "PATCH", &page_url, Some(json!({"is_hidden": true})),
        Some(&secret), None).await;

    // Hiding mints NO share link: page is owner-only.
    let owner = call(&app, "GET", &page_url, None, Some(&secret), None).await;
    assert_eq!(owner.body["is_hidden"], json!(true));
    assert_eq!(owner.body["share_token"], Value::Null);

    // Canonical identifiers are dead to visitors.
    for ident in [slug.as_str(), "ghostwriter"] {
        let api_get = call(&app, "GET", &format!("/api/pages/{ident}"), None, None, None).await;
        assert_eq!(api_get.status, StatusCode::NOT_FOUND, "api {ident}");
        let html = call(&app, "GET", &format!("/u/{ident}"), None, None, None).await;
        assert_eq!(html.status, StatusCode::NOT_FOUND, "html {ident}");
        let submit = call(&app, "POST", &format!("/api/pages/{ident}/recommendations"),
            Some(json!({"book_name": "The Dispossessed"})), None, None).await;
        assert_eq!(submit.status, StatusCode::NOT_FOUND, "submit {ident}");
    }

    // The owner door serves the shell for ANY slug, uniformly (no leak).
    let unlock = call(&app, "GET", &format!("/u/{slug}?unlock"), None, None, None).await;
    assert_eq!(unlock.status, StatusCode::OK);
    let unlock_garbage = call(&app, "GET", "/u/never-existed?unlock", None, None, None).await;
    assert_eq!(unlock_garbage.status, StatusCode::OK);

    // Mint a friend link on request; it opens the page for visitors.
    let minted = call(&app, "POST", &format!("/api/pages/{slug}/share-link"),
        None, Some(&secret), None).await;
    assert_eq!(minted.status, StatusCode::OK);
    let token = minted.body["share_token"].as_str().unwrap().to_string();
    let via_token = call(&app, "GET", &format!("/api/pages/{token}"), None, None, None).await;
    assert_eq!(via_token.status, StatusCode::OK);
    let html = call(&app, "GET", &format!("/u/{token}"), None, None, None).await;
    assert_eq!(html.status, StatusCode::OK);

    // Regenerating rotates: old token dies, new one works.
    let rotated = call(&app, "POST", &format!("/api/pages/{slug}/share-link"),
        None, Some(&secret), None).await;
    let token2 = rotated.body["share_token"].as_str().unwrap().to_string();
    assert_ne!(token, token2);
    let old = call(&app, "GET", &format!("/api/pages/{token}"), None, None, None).await;
    assert_eq!(old.status, StatusCode::NOT_FOUND);
    let fresh = call(&app, "GET", &format!("/api/pages/{token2}"), None, None, None).await;
    assert_eq!(fresh.status, StatusCode::OK);

    // Revoking kills the link entirely; owner access survives.
    let revoked = call(&app, "DELETE", &format!("/api/pages/{slug}/share-link"),
        None, Some(&secret), None).await;
    assert_eq!(revoked.status, StatusCode::OK);
    assert_eq!(revoked.body["handle"], json!("ghostwriter"));
    let dead = call(&app, "GET", &format!("/api/pages/{token2}"), None, None, None).await;
    assert_eq!(dead.status, StatusCode::NOT_FOUND);
    let owner = call(&app, "GET", &page_url, None, Some(&secret), None).await;
    assert_eq!(owner.status, StatusCode::OK);

    // Unhide restores public access via canonical URLs.
    call(&app, "PATCH", &page_url, Some(json!({"is_hidden": false})),
        Some(&secret), None).await;
    let public = call(&app, "GET", "/api/pages/ghostwriter", None, None, None).await;
    assert_eq!(public.status, StatusCode::OK);
}

#[tokio::test]
async fn card_page_404s_for_unknown_slug() {
    let (app, _pool) = test_app().await;
    let (slug, _) = create_page(&app).await;

    let good = call(&app, "GET", &format!("/u/{slug}"), None, None, None).await;
    assert_eq!(good.status, StatusCode::OK);

    let bad = call(&app, "GET", "/u/definitely-not-a-slug", None, None, None).await;
    assert_eq!(bad.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn submit_by_title_and_isbn() {
    let (app, _pool) = test_app().await;
    let (slug, _) = create_page(&app).await;
    let submit_url = format!("/api/pages/{slug}/recommendations");

    let by_title = call(&app, "POST", &submit_url,
        Some(json!({"book_name": "the dispossessed"})), None, None).await;
    assert_eq!(by_title.status, StatusCode::OK);
    assert_eq!(by_title.body["status"], json!("created"));
    assert_eq!(by_title.body["book"]["title"], json!("The Dispossessed"));
    assert!(by_title.cookie.is_some(), "first submission mints an anon cookie");

    // ISBN-10 input for the known ISBN-13 (978-0-13-468599-1 → 0-13-468599-7... use 13)
    let by_isbn = call(&app, "POST", &submit_url,
        Some(json!({"book_name": "978-0-13-468599-1"})), None, None).await;
    assert_eq!(by_isbn.status, StatusCode::OK);
    assert_eq!(by_isbn.body["book"]["title"], json!(KNOWN_ISBN_TITLE));

    let page = call(&app, "GET", &format!("/api/pages/{slug}"), None, None, None).await;
    assert_eq!(page.body["recommendations"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn rejects_garbage() {
    let (app, _pool) = test_app().await;
    let (slug, _) = create_page(&app).await;
    let submit_url = format!("/api/pages/{slug}/recommendations");

    let bad_checksum = call(&app, "POST", &submit_url,
        Some(json!({"book_name": "9780134685992"})), None, None).await;
    assert_eq!(bad_checksum.status, StatusCode::BAD_REQUEST);

    let unknown_isbn = call(&app, "POST", &submit_url,
        Some(json!({"book_name": TITLE_MATCH_ISBN})), None, None).await;
    assert_eq!(unknown_isbn.status, StatusCode::BAD_REQUEST);

    let unknown_title = call(&app, "POST", &submit_url,
        Some(json!({"book_name": "definitely not a real book"})), None, None).await;
    assert_eq!(unknown_title.status, StatusCode::BAD_REQUEST);

    let empty = call(&app, "POST", &submit_url,
        Some(json!({"book_name": "   "})), None, None).await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn dedupe_counts_once_per_submitter() {
    let (app, _pool) = test_app().await;
    let (slug, _) = create_page(&app).await;
    let submit_url = format!("/api/pages/{slug}/recommendations");
    let body = json!({"book_name": "The Dispossessed"});

    let first = call(&app, "POST", &submit_url, Some(body.clone()), None, None).await;
    assert_eq!(first.body["status"], json!("created"));
    let cookie = first.cookie.unwrap();

    // Same submitter again: no new count.
    let again = call(&app, "POST", &submit_url, Some(body.clone()), None, Some(&cookie)).await;
    assert_eq!(again.body["status"], json!("already_recommended"));

    // A different submitter bumps the count instead of creating a row.
    let other = call(&app, "POST", &submit_url, Some(body.clone()), None, None).await;
    assert_eq!(other.body["status"], json!("counted"));

    let page = call(&app, "GET", &format!("/api/pages/{slug}"), None, None, None).await;
    let recs = page.body["recommendations"].as_array().unwrap();
    assert_eq!(recs.len(), 1, "duplicates must never create a second row");
    assert_eq!(recs[0]["recommendation_count"], json!(2));
}

#[tokio::test]
async fn private_recommendations_are_encrypted_at_rest() {
    let (app, pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;

    let patched = call(&app, "PATCH", &format!("/api/pages/{slug}"),
        Some(json!({"default_public": false})), Some(&secret), None).await;
    assert_eq!(patched.status, StatusCode::OK);

    let submitted = call(&app, "POST", &format!("/api/pages/{slug}/recommendations"),
        Some(json!({"book_name": "The Dispossessed"})), None, None).await;
    assert_eq!(submitted.body["status"], json!("created"));

    // At rest: no plaintext, sealed payload present.
    let (book_name, isbn, author, enc_len): (Option<String>, Option<String>, Option<String>, i64) =
        sqlx::query_as(
        "SELECT book_name, isbn, author, length(enc_payload) FROM recommendations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_name, None);
    assert_eq!(isbn, None);
    assert_eq!(author, None, "author must be sealed, not plaintext");
    assert!(enc_len > 0);
    let raw: Vec<u8> = sqlx::query_scalar("SELECT enc_payload FROM recommendations")
        .fetch_one(&pool)
        .await
        .unwrap();
    let raw_text = String::from_utf8_lossy(&raw);
    assert!(!raw_text.contains("Dispossessed"), "sealed payload must not leak the title");

    // Public view: invisible. Owner view: decrypted.
    let public = call(&app, "GET", &format!("/api/pages/{slug}"), None, None, None).await;
    assert_eq!(public.body["recommendations"].as_array().unwrap().len(), 0);

    let owner = call(&app, "GET", &format!("/api/pages/{slug}"), None, Some(&secret), None).await;
    let recs = owner.body["recommendations"].as_array().unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["book_name"], json!("The Dispossessed"));
    assert_eq!(recs[0]["author"], json!("Ursula K. Le Guin"));
    assert_eq!(recs[0]["is_public"], json!(false));

    // Dedupe still works against the sealed row.
    let other = call(&app, "POST", &format!("/api/pages/{slug}/recommendations"),
        Some(json!({"book_name": "the dispossessed"})), None, None).await;
    assert_eq!(other.body["status"], json!("counted"));
}

#[tokio::test]
async fn owner_can_toggle_visibility_read_and_delete() {
    let (app, pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let submit_url = format!("/api/pages/{slug}/recommendations");

    call(&app, "POST", &submit_url, Some(json!({"book_name": "The Dispossessed"})), None, None)
        .await;
    let page = call(&app, "GET", &format!("/api/pages/{slug}"), None, Some(&secret), None).await;
    let rec_id = page.body["recommendations"][0]["id"].as_i64().unwrap();
    let rec_url = format!("/api/recommendations/{rec_id}");

    // No bearer → 401; visitor cannot manage anything.
    let denied = call(&app, "PATCH", &rec_url, Some(json!({"read": true})), None, None).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    let denied = call(&app, "DELETE", &rec_url, None, Some("wrong"), None).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);

    // Mark read.
    let read = call(&app, "PATCH", &rec_url, Some(json!({"read": true})), Some(&secret), None).await;
    assert_eq!(read.status, StatusCode::OK);

    // Seal a public rec: plaintext must vanish from the row.
    call(&app, "PATCH", &rec_url, Some(json!({"is_public": false})), Some(&secret), None).await;
    let (book_name, enc_len): (Option<String>, i64) =
        sqlx::query_as("SELECT book_name, length(enc_payload) FROM recommendations WHERE id = ?")
            .bind(rec_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(book_name, None);
    assert!(enc_len > 0);
    let public = call(&app, "GET", &format!("/api/pages/{slug}"), None, None, None).await;
    assert_eq!(public.body["recommendations"].as_array().unwrap().len(), 0);

    // Reveal it again: plaintext restored, read flag survived.
    call(&app, "PATCH", &rec_url, Some(json!({"is_public": true})), Some(&secret), None).await;
    let public = call(&app, "GET", &format!("/api/pages/{slug}"), None, None, None).await;
    let recs = public.body["recommendations"].as_array().unwrap();
    assert_eq!(recs[0]["book_name"], json!("The Dispossessed"));
    assert_eq!(recs[0]["author"], json!("Ursula K. Le Guin"));
    assert_eq!(recs[0]["read"], json!(true));

    // Delete.
    let deleted = call(&app, "DELETE", &rec_url, None, Some(&secret), None).await;
    assert_eq!(deleted.status, StatusCode::OK);
    let page = call(&app, "GET", &format!("/api/pages/{slug}"), None, Some(&secret), None).await;
    assert_eq!(page.body["recommendations"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn submitter_notes_follow_recommendation_visibility() {
    let (app, pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let submit_url = format!("/api/pages/{slug}/recommendations");
    let page_url = format!("/api/pages/{slug}");

    // Public rec: note is plaintext and publicly visible.
    call(&app, "POST", &submit_url,
        Some(json!({"book_name": "The Dispossessed", "note": "changed how I think"})),
        None, None).await;
    let public = call(&app, "GET", &page_url, None, None, None).await;
    assert_eq!(public.body["recommendations"][0]["notes"], json!(["changed how I think"]));

    // Second submitter's note joins the same row.
    call(&app, "POST", &submit_url,
        Some(json!({"book_name": "the dispossessed", "note": "an anarchist moon!"})),
        None, None).await;
    let public = call(&app, "GET", &page_url, None, None, None).await;
    assert_eq!(public.body["recommendations"][0]["notes"],
        json!(["changed how I think", "an anarchist moon!"]));

    // Sealing the rec seals the notes too.
    let rec_id = public.body["recommendations"][0]["id"].as_i64().unwrap();
    call(&app, "PATCH", &format!("/api/recommendations/{rec_id}"),
        Some(json!({"is_public": false})), Some(&secret), None).await;
    let (plain, sealed): (i64, i64) = sqlx::query_as(
        "SELECT count(note), count(enc_note) FROM recommendation_votes
         WHERE recommendation_id = ?")
        .bind(rec_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((plain, sealed), (0, 2), "notes must be sealed with the rec");

    // Owner still reads them; a new note to the sealed rec is sealed on arrival.
    call(&app, "POST", &submit_url,
        Some(json!({"book_name": "The Dispossessed", "note": "sealed note"})), None, None).await;
    let owner = call(&app, "GET", &page_url, None, Some(&secret), None).await;
    assert_eq!(owner.body["recommendations"][0]["notes"],
        json!(["changed how I think", "an anarchist moon!", "sealed note"]));
    let public = call(&app, "GET", &page_url, None, None, None).await;
    assert_eq!(public.body["recommendations"].as_array().unwrap().len(), 0);

    // Revealing brings all notes back to plaintext.
    call(&app, "PATCH", &format!("/api/recommendations/{rec_id}"),
        Some(json!({"is_public": true})), Some(&secret), None).await;
    let public = call(&app, "GET", &page_url, None, None, None).await;
    assert_eq!(public.body["recommendations"][0]["notes"].as_array().unwrap().len(), 3);

    // Oversized note rejected.
    let too_long = call(&app, "POST", &submit_url,
        Some(json!({"book_name": "9780134685991", "note": "x".repeat(501)})), None, None).await;
    assert_eq!(too_long.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn display_name_is_optional_and_owner_set() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let page_url = format!("/api/pages/{slug}");

    let anonymous = call(&app, "GET", &page_url, None, None, None).await;
    assert_eq!(anonymous.body["display_name"], Value::Null);

    // Only the owner can set it.
    let denied = call(&app, "PATCH", &page_url,
        Some(json!({"display_name": "Lachy"})), None, None).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);

    let set = call(&app, "PATCH", &page_url,
        Some(json!({"display_name": "  Lachy  "})), Some(&secret), None).await;
    assert_eq!(set.status, StatusCode::OK);

    // Visitors see the trimmed name; the slug (URL) is unchanged.
    let visitor = call(&app, "GET", &page_url, None, None, None).await;
    assert_eq!(visitor.body["display_name"], json!("Lachy"));
    assert_eq!(visitor.body["slug"], json!(slug));

    let too_long = call(&app, "PATCH", &page_url,
        Some(json!({"display_name": "x".repeat(65)})), Some(&secret), None).await;
    assert_eq!(too_long.status, StatusCode::BAD_REQUEST);

    // Empty string clears it.
    call(&app, "PATCH", &page_url, Some(json!({"display_name": ""})), Some(&secret), None).await;
    let cleared = call(&app, "GET", &page_url, None, None, None).await;
    assert_eq!(cleared.body["display_name"], Value::Null);
}

#[tokio::test]
async fn profile_only_shown_for_public_pages() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let page_url = format!("/api/pages/{slug}");

    let set = call(&app, "PATCH", &page_url,
        Some(json!({"bio": "I read at night", "favourite_book": "The Dispossessed",
                    "theme": "plum"})),
        Some(&secret), None).await;
    assert_eq!(set.status, StatusCode::OK);

    let bad_theme = call(&app, "PATCH", &page_url,
        Some(json!({"theme": "neon"})), Some(&secret), None).await;
    assert_eq!(bad_theme.status, StatusCode::BAD_REQUEST);

    // Style options round-trip and validate.
    let styled = call(&app, "PATCH", &page_url,
        Some(json!({"font": "typewriter", "radius": "pill", "density": "compact",
                    "accent": "#A1B2C3"})),
        Some(&secret), None).await;
    assert_eq!(styled.status, StatusCode::OK);
    let bad_font = call(&app, "PATCH", &page_url,
        Some(json!({"font": "comic-sans"})), Some(&secret), None).await;
    assert_eq!(bad_font.status, StatusCode::BAD_REQUEST);
    let bad_accent = call(&app, "PATCH", &page_url,
        Some(json!({"accent": "red"})), Some(&secret), None).await;
    assert_eq!(bad_accent.status, StatusCode::BAD_REQUEST);

    // Public page: visitors see the profile.
    let visitor = call(&app, "GET", &page_url, None, None, None).await;
    assert_eq!(visitor.body["font"], json!("typewriter"));
    assert_eq!(visitor.body["accent"], json!("#a1b2c3"));
    assert_eq!(visitor.body["radius"], json!("pill"));
    assert_eq!(visitor.body["bio"], json!("I read at night"));
    assert_eq!(visitor.body["favourite_book"], json!("The Dispossessed"));
    assert_eq!(visitor.body["theme"], json!("plum"));

    // Private page: profile hidden from visitors, still there for the owner.
    call(&app, "PATCH", &page_url, Some(json!({"default_public": false})),
        Some(&secret), None).await;
    let visitor = call(&app, "GET", &page_url, None, None, None).await;
    assert_eq!(visitor.body["bio"], Value::Null);
    assert_eq!(visitor.body["favourite_book"], Value::Null);
    assert_eq!(visitor.body["theme"], Value::Null);
    let owner = call(&app, "GET", &page_url, None, Some(&secret), None).await;
    assert_eq!(owner.body["bio"], json!("I read at night"));
}

#[tokio::test]
async fn account_deletion_requires_owner_and_removes_everything() {
    let (app, pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let page_url = format!("/api/pages/{slug}");

    call(&app, "POST", &format!("{page_url}/recommendations"),
        Some(json!({"book_name": "The Dispossessed", "note": "gone soon"})), None, None).await;

    let denied = call(&app, "DELETE", &page_url, None, None, None).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    let denied = call(&app, "DELETE", &page_url, None, Some("wrong"), None).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);

    let deleted = call(&app, "DELETE", &page_url, None, Some(&secret), None).await;
    assert_eq!(deleted.status, StatusCode::OK);

    let gone = call(&app, "GET", &page_url, None, Some(&secret), None).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);

    // Cascade wiped every trace.
    let (users, recs, votes): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM users),
                (SELECT count(*) FROM recommendations),
                (SELECT count(*) FROM recommendation_votes)")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((users, recs, votes), (0, 0, 0));
}

#[tokio::test]
async fn key_rotation_invalidates_old_key_and_keeps_sealed_data() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let page_url = format!("/api/pages/{slug}");
    let rotate_url = format!("{page_url}/rotate-key");

    // A sealed recommendation that must survive the rotation.
    call(&app, "PATCH", &page_url, Some(json!({"default_public": false})),
        Some(&secret), None).await;
    call(&app, "POST", &format!("{page_url}/recommendations"),
        Some(json!({"book_name": "The Dispossessed", "note": "sealed for later"})),
        None, None).await;

    // Signed-in only: no bearer / wrong bearer are rejected.
    let denied = call(&app, "POST", &rotate_url, None, None, None).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    let denied = call(&app, "POST", &rotate_url, None, Some("wrong"), None).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);

    let rotated = call(&app, "POST", &rotate_url, None, Some(&secret), None).await;
    assert_eq!(rotated.status, StatusCode::OK);
    let new_secret = rotated.body["secret"].as_str().unwrap().to_string();
    assert_ne!(new_secret, secret);

    // Old key is dead — for reads and for a second rotation.
    let stale = call(&app, "GET", &page_url, None, Some(&secret), None).await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);
    let stale = call(&app, "POST", &rotate_url, None, Some(&secret), None).await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);

    // New key owns the page and still decrypts the sealed recommendation.
    let owner = call(&app, "GET", &page_url, None, Some(&new_secret), None).await;
    assert_eq!(owner.status, StatusCode::OK);
    assert_eq!(owner.body["owner"], json!(true));
    let recs = owner.body["recommendations"].as_array().unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["book_name"], json!("The Dispossessed"));
    assert_eq!(recs[0]["notes"], json!(["sealed for later"]));
}

#[tokio::test]
async fn qr_endpoint_serves_share_url_svg() {
    let (app, _pool) = test_app().await;
    let (slug, _) = create_page(&app).await;

    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/pages/{slug}/qr.svg"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/svg+xml"
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let svg = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(svg.contains("<svg"), "response must be an SVG document");

    let missing = call(&app, "GET", "/api/pages/doesnotexist/qr.svg", None, None, None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_paths_get_styled_404() {
    let (app, _pool) = test_app().await;
    let request = Request::builder()
        .method("GET")
        .uri("/definitely/not/here")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains("Page not found"), "404 body must be the styled page");
}

#[tokio::test]
async fn search_returns_suggestions_with_metadata() {
    let (app, _pool) = test_app().await;

    let found = call(&app, "GET", "/api/search?q=dispossessed", None, None, None).await;
    assert_eq!(found.status, StatusCode::OK);
    let suggestions = found.body.as_array().unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0]["title"], json!("The Dispossessed"));
    assert_eq!(suggestions[0]["author"], json!("Ursula K. Le Guin"));
    assert_eq!(suggestions[0]["isbn13"], json!(TITLE_MATCH_ISBN));
    assert_eq!(suggestions[0]["cover_id"], json!(12345));

    let none = call(&app, "GET", "/api/search?q=nothing+here", None, None, None).await;
    assert_eq!(none.body.as_array().unwrap().len(), 0);

    let empty = call(&app, "GET", "/api/search?q=", None, None, None).await;
    assert_eq!(empty.body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn same_book_on_different_pages_is_independent() {
    let (app, _pool) = test_app().await;
    let (slug_a, _) = create_page(&app).await;
    let (slug_b, _) = create_page(&app).await;
    let body = json!({"book_name": "The Dispossessed"});

    let a = call(&app, "POST", &format!("/api/pages/{slug_a}/recommendations"),
        Some(body.clone()), None, None).await;
    let b = call(&app, "POST", &format!("/api/pages/{slug_b}/recommendations"),
        Some(body.clone()), None, None).await;
    assert_eq!(a.body["status"], json!("created"));
    assert_eq!(b.body["status"], json!("created"), "blind index must be page-scoped");
}

#[tokio::test]
async fn card_pages_carry_link_preview_meta() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    call(&app, "PATCH", &format!("/api/pages/{slug}"),
        Some(json!({"display_name": "Lachy <script>"})), Some(&secret), None).await;

    let request = Request::builder()
        .method("GET")
        .uri(format!("/u/{slug}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&bytes).to_string();

    assert!(html.contains("Recommend Lachy &lt;script&gt; a book"), "escaped name in meta");
    assert!(html.contains(r#"property="og:url""#));
    assert!(!html.contains("Lachy <script>"), "unescaped name must never appear");

    // Hidden pages keep the generic meta on their friend link.
    call(&app, "PATCH", &format!("/api/pages/{slug}"),
        Some(json!({"is_hidden": true})), Some(&secret), None).await;
    let minted = call(&app, "POST", &format!("/api/pages/{slug}/share-link"),
        None, Some(&secret), None).await;
    let token = minted.body["share_token"].as_str().unwrap();
    let request = Request::builder()
        .method("GET")
        .uri(format!("/u/{token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&bytes).to_string();
    assert!(!html.contains("Lachy"), "hidden page must not leak the name in meta");
    assert!(html.contains(r#"og:title" content="Recommend a thing""#));
}

#[tokio::test]
async fn friend_link_allows_recommending() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    call(&app, "PATCH", &format!("/api/pages/{slug}"),
        Some(json!({"is_hidden": true})), Some(&secret), None).await;
    let minted = call(&app, "POST", &format!("/api/pages/{slug}/share-link"),
        None, Some(&secret), None).await;
    let token = minted.body["share_token"].as_str().unwrap().to_string();

    let submit = call(&app, "POST", &format!("/api/pages/{token}/recommendations"),
        Some(json!({"book_name": "The Dispossessed", "note": "via friend link"})),
        None, None).await;
    assert_eq!(submit.status, StatusCode::OK, "body: {:?}", submit.body);
    assert_eq!(submit.body["status"], json!("created"));

    // Owner submitting on the canonical URL: allowed because the frontend
    // sends the bearer key with the submission.
    let owner_submit = call(&app, "POST", &format!("/api/pages/{slug}/recommendations"),
        Some(json!({"book_name": "9780134685991"})), Some(&secret), None).await;
    assert_eq!(owner_submit.status, StatusCode::OK, "body: {:?}", owner_submit.body);
}

#[tokio::test]
async fn banning_removes_and_blocks_resubmission() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    let submit_url = format!("/api/pages/{slug}/recommendations");
    let body = json!({"book_name": "The Dispossessed"});

    call(&app, "POST", &submit_url, Some(body.clone()), None, None).await;
    let page = call(&app, "GET", &format!("/api/pages/{slug}"), None, Some(&secret), None).await;
    let rec_id = page.body["recommendations"][0]["id"].as_i64().unwrap();

    let denied = call(&app, "POST", &format!("/api/recommendations/{rec_id}/ban"),
        None, None, None).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);

    let banned = call(&app, "POST", &format!("/api/recommendations/{rec_id}/ban"),
        None, Some(&secret), None).await;
    assert_eq!(banned.status, StatusCode::OK);
    assert_eq!(banned.body["isbn"], json!(TITLE_MATCH_ISBN));

    // Rec gone, resubmission rejected, ban listed.
    let page = call(&app, "GET", &format!("/api/pages/{slug}"), None, Some(&secret), None).await;
    assert_eq!(page.body["recommendations"].as_array().unwrap().len(), 0);
    let again = call(&app, "POST", &submit_url, Some(body.clone()), None, None).await;
    assert_eq!(again.status, StatusCode::BAD_REQUEST);
    let bans = call(&app, "GET", &format!("/api/pages/{slug}/bans"), None, Some(&secret), None).await;
    assert_eq!(bans.body["bans"][0]["isbn"], json!(TITLE_MATCH_ISBN));

    // Unban restores submissions.
    let unbanned = call(&app, "DELETE",
        &format!("/api/pages/{slug}/bans/{TITLE_MATCH_ISBN}"), None, Some(&secret), None).await;
    assert_eq!(unbanned.status, StatusCode::OK);
    let again = call(&app, "POST", &submit_url, Some(body), None, None).await;
    assert_eq!(again.body["status"], json!("created"));
}

#[tokio::test]
async fn rss_feed_lists_public_recs_only() {
    let (app, _pool) = test_app().await;
    let (slug, secret) = create_page(&app).await;
    call(&app, "POST", &format!("/api/pages/{slug}/recommendations"),
        Some(json!({"book_name": "The Dispossessed"})), None, None).await;

    let request = Request::builder()
        .method("GET")
        .uri(format!("/u/{slug}/feed.xml"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap().to_string();
    assert!(content_type.starts_with("application/rss+xml"));
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let xml = String::from_utf8_lossy(&bytes).to_string();
    assert!(xml.contains("The Dispossessed — Ursula K. Le Guin"));
    assert!(xml.contains("<pubDate>"));

    // Hidden page: canonical feed 404s.
    call(&app, "PATCH", &format!("/api/pages/{slug}"),
        Some(json!({"is_hidden": true})), Some(&secret), None).await;
    let request = Request::builder()
        .method("GET")
        .uri(format!("/u/{slug}/feed.xml"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resolver_cache_prevents_repeat_lookups() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingResolver(AtomicUsize);
    #[async_trait::async_trait]
    impl BookResolver for CountingResolver {
        async fn by_isbn(&self, isbn13: &str) -> Result<Option<ResolvedBook>, ResolverError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Some(ResolvedBook {
                title: "Counted".into(),
                isbn13: isbn13.to_string(),
                author: None,
            }))
        }
        async fn by_title(&self, _: &str) -> Result<Option<ResolvedBook>, ResolverError> {
            Ok(None)
        }
        async fn search(&self, _: &str) -> Result<Vec<BookSuggestion>, ResolverError> {
            Ok(Vec::new())
        }
    }

    let options = SqliteConnectOptions::from_str("sqlite::memory:").unwrap();
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();

    let counter = Arc::new(CountingResolver(AtomicUsize::new(0)));
    let cached = anonbookrec::resolver::CachingResolver::new(counter.clone(), pool);

    for _ in 0..3 {
        let hit = cached.by_isbn(KNOWN_ISBN).await.unwrap().unwrap();
        assert_eq!(hit.title, "Counted");
    }
    assert_eq!(counter.0.load(Ordering::SeqCst), 1, "only the first lookup hits upstream");
}
