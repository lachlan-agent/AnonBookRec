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
        }))
    }

    async fn by_title(&self, title: &str) -> Result<Option<ResolvedBook>, ResolverError> {
        Ok(title.to_lowercase().contains("dispossessed").then(|| ResolvedBook {
            title: "The Dispossessed".to_string(),
            isbn13: TITLE_MATCH_ISBN.to_string(),
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

async fn create_page(app: &Router) -> (String, String) {
    let response = call(app, "POST", "/api/users", None, None, None).await;
    assert_eq!(response.status, StatusCode::OK);
    (
        response.body["slug"].as_str().unwrap().to_string(),
        response.body["secret"].as_str().unwrap().to_string(),
    )
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
    let (book_name, isbn, enc_len): (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT book_name, isbn, length(enc_payload) FROM recommendations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_name, None);
    assert_eq!(isbn, None);
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
    assert_eq!(recs[0]["read"], json!(true));

    // Delete.
    let deleted = call(&app, "DELETE", &rec_url, None, Some(&secret), None).await;
    assert_eq!(deleted.status, StatusCode::OK);
    let page = call(&app, "GET", &format!("/api/pages/{slug}"), None, Some(&secret), None).await;
    assert_eq!(page.body["recommendations"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn global_board_visit_and_submit() {
    let (app, _pool) = test_app().await;

    let submitted = call(&app, "POST", "/api/global/recommendations",
        Some(json!({"book_name": "The Dispossessed"})), None, None).await;
    assert_eq!(submitted.status, StatusCode::OK);
    assert_eq!(submitted.body["status"], json!("created"));

    let board = call(&app, "GET", "/api/global", None, None, None).await;
    let recs = board.body.as_array().unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["book_name"], json!("The Dispossessed"));

    // Global rows have no owner: nobody can patch or delete them.
    let rec_id = recs[0]["id"].as_i64().unwrap();
    let patched = call(&app, "PATCH", &format!("/api/recommendations/{rec_id}"),
        Some(json!({"read": true})), Some("anything"), None).await;
    assert_eq!(patched.status, StatusCode::NOT_FOUND);
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

    // Public page: visitors see the profile.
    let visitor = call(&app, "GET", &page_url, None, None, None).await;
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
