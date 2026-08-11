use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{any, get, patch, post};
use axum::Router;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_governor::GovernorLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::routes::{recommendations, users};
use crate::state::AppState;

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Swap the generic meta block in user.html for a personalized one so link
/// previews (Instagram, iMessage, Slack…) show whose page it is.
fn personalize_meta(html: &str, display_name: Option<&str>, base_url: &str, url: &str) -> String {
    let (Some(start), Some(end)) = (html.find("<!--META-->"), html.find("<!--/META-->")) else {
        return html.to_string();
    };
    let title = match display_name {
        Some(name) => format!("Recommend {} a book", escape_html(name)),
        None => "Recommend a book".to_string(),
    };
    let meta = format!(
        r#"<!--META-->
<title>{title}</title>
<meta property="og:type" content="website">
<meta property="og:site_name" content="Suggest a Thing">
<meta property="og:title" content="{title}">
<meta property="og:description" content="Suggest this reader their next book — anonymously. No account needed.">
<meta property="og:url" content="{url}">
<meta property="og:image" content="{image}">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:image" content="{image}">
"#,
        url = escape_html(url),
        // The ?v= busts Meta's per-URL scrape cache when the card changes.
        image = escape_html(&format!("{base_url}/og.png?v=2")),
    );
    format!("{}{}{}", &html[..start], meta, &html[end..])
}

pub fn build_app(state: AppState) -> Router {
    let mut read_api = Router::new()
        .route("/api/pages/{slug}", get(users::get_page))
        .route("/api/pages/{slug}/bans", get(users::list_bans))
        .route("/u/{slug}/feed.xml", get(users::feed))
        .route("/api/pages/{slug}/qr.svg", get(users::qr_svg))
        .route("/api/search", get(recommendations::search));

    // Mutating routes carry the per-IP rate limit.
    let mut write_api = Router::new()
        .route("/api/users/draft", post(users::draft_page))
        .route("/api/users", post(users::create_page))
        .route("/api/pages/{slug}", patch(users::patch_page).delete(users::delete_page))
        .route("/api/pages/{slug}/rotate-key", post(users::rotate_key))
        .route(
            "/api/pages/{slug}/credentials",
            post(users::set_credentials).delete(users::delete_credentials),
        )
        .route(
            "/api/pages/{slug}/share-link",
            post(users::create_share_link).delete(users::revoke_share_link),
        )
        .route("/api/pages/{slug}/recommendations", post(recommendations::submit_to_page))
        .route(
            "/api/recommendations/{id}",
            patch(recommendations::patch_rec).delete(recommendations::delete_rec),
        )
        .route("/api/recommendations/{id}/ban", post(recommendations::ban_rec))
        .route("/api/pages/{slug}/bans/{isbn}", axum::routing::delete(users::unban));

    // Password guessing gets its own, much stricter per-IP budget on top of
    // Argon2id's inherent slowness.
    let mut login_api = Router::new().route("/api/login", post(users::login));
    if state.config.rate_limiting {
        let login_governor = GovernorConfigBuilder::default()
            .key_extractor(SmartIpKeyExtractor)
            .period(std::time::Duration::from_secs(10)) // refill: 1 attempt / 10 s
            .burst_size(5)
            .finish()
            .expect("valid governor config");
        login_api = login_api.layer(GovernorLayer::new(Arc::new(login_governor)));
    }

    if state.config.rate_limiting {
        let governor = GovernorConfigBuilder::default()
            .key_extractor(SmartIpKeyExtractor)
            .per_second(6)
            .burst_size(10)
            .finish()
            .expect("valid governor config");
        write_api = write_api.layer(GovernorLayer::new(Arc::new(governor)));

        // Reads get a laxer per-IP limit; mainly so /api/search can't be used
        // to relay unbounded traffic at Open Library.
        let read_governor = GovernorConfigBuilder::default()
            .key_extractor(SmartIpKeyExtractor)
            .per_second(15)
            .burst_size(40)
            .finish()
            .expect("valid governor config");
        read_api = read_api.layer(GovernorLayer::new(Arc::new(read_governor)));
    }

    // API responses carry per-request data (owner-decrypted payloads on the
    // page endpoint); never let an intermediary cache them. Handlers that are
    // safe to cache (the QR SVG) set their own Cache-Control first.
    let no_store = SetResponseHeaderLayer::if_not_present(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    read_api = read_api.layer(no_store.clone());
    write_api = write_api.layer(no_store.clone());
    login_api = login_api.layer(no_store);

    let static_dir = &state.config.static_dir;
    let about_page = ServeFile::new(static_dir.join("about.html"));
    // Unknown paths get the styled 404 page with a real 404 status (serving
    // the file directly would answer 200). Read once at startup.
    let not_found_html = std::fs::read_to_string(static_dir.join("404.html"))
        .unwrap_or_else(|_| "<h1>Page not found</h1>".to_string());
    let user_html = std::fs::read_to_string(static_dir.join("user.html"))
        .unwrap_or_else(|_| "<h1>missing user.html</h1>".to_string());

    // A card page for a slug that doesn't exist is a 404, not the app shell
    // with a broken fetch inside it. Check the slug before serving the shell.
    let user_page = get({
        let state = state.clone();
        let user_html = user_html.clone();
        let not_found_html = not_found_html.clone();
        move |axum::extract::Path(slug): axum::extract::Path<String>,
              axum::extract::RawQuery(query): axum::extract::RawQuery| {
            let state = state.clone();
            let user_html = user_html.clone();
            let not_found_html = not_found_html.clone();
            async move {
                // `?unlock` serves the shell for ANY slug — identical response
                // whether the page exists, is hidden, or never existed, so it
                // leaks nothing. The API still gates all data; this is the
                // owner's door into a hidden page (document loads can't carry
                // a bearer). The 404 page auto-redirects here when the
                // browser holds a key for the path.
                if query.as_deref().is_some_and(|q| q.split('&').any(|p| p == "unlock")) {
                    return Html(user_html).into_response();
                }
                match crate::auth::user_by_slug(&state.pool, &slug).await {
                    // A hidden page's shell is otherwise served only at its
                    // share-token URL; every other identifier 404s exactly
                    // like a missing page.
                    Ok(Some(user))
                        if !user.is_hidden
                            || user.share_token.as_deref() == Some(slug.as_str()) =>
                    {
                        // Personalized link-preview meta (Open Graph). Hidden
                        // pages keep the generic block — a shared friend link
                        // shouldn't announce whose page it is.
                        let html = if user.is_hidden {
                            user_html
                        } else {
                            personalize_meta(
                                &user_html,
                                user.display_name.as_deref(),
                                &state.config.public_base_url,
                                &format!(
                                    "{}/u/{}",
                                    state.config.public_base_url,
                                    user.handle()
                                ),
                            )
                        };
                        Html(html).into_response()
                    }
                    Ok(Some(_)) | Ok(None) => {
                        (StatusCode::NOT_FOUND, Html(not_found_html)).into_response()
                    }
                    Err(err) => err.into_response(),
                }
            }
        }
    });

    let not_found_page = any(move || {
        let html = not_found_html.clone();
        async move { (StatusCode::NOT_FOUND, Html(html)) }
    });
    let static_files = ServeDir::new(static_dir).not_found_service(not_found_page);

    let mut app = Router::new()
        .merge(read_api)
        .merge(write_api)
        .merge(login_api)
        .route("/u/{slug}", user_page)
        .route_service("/about", about_page)
        .fallback_service(static_files)
        // Static assets revalidate on every load (ServeDir answers 304s), so
        // a deploy can never leave a browser running old HTML against new JS.
        // API routes already carry no-store; the QR sets its own max-age.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(DefaultBodyLimit::max(4096))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' data: https://covers.openlibrary.org; \
                 object-src 'none'; base-uri 'none'; form-action 'self'; \
                 frame-ancestors 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ));

    // Only meaningful (and only sent) when the deployment runs behind HTTPS,
    // which is what SECURE_COOKIES signals.
    if state.config.secure_cookies {
        app = app.layer(SetResponseHeaderLayer::if_not_present(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ));
    }

    app.with_state(state)
}
