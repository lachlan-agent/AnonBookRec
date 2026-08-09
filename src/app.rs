use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue};
use axum::routing::{get, patch, post};
use axum::Router;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_governor::GovernorLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::routes::{recommendations, users};
use crate::state::AppState;

pub fn build_app(state: AppState) -> Router {
    let mut read_api = Router::new()
        .route("/api/pages/{slug}", get(users::get_page))
        .route("/api/pages/{slug}/qr.svg", get(users::qr_svg))
        .route("/api/search", get(recommendations::search));

    // Mutating routes carry the per-IP rate limit.
    let mut write_api = Router::new()
        .route("/api/users", post(users::create_page))
        .route("/api/pages/{slug}", patch(users::patch_page).delete(users::delete_page))
        .route("/api/pages/{slug}/rotate-key", post(users::rotate_key))
        .route("/api/pages/{slug}/recommendations", post(recommendations::submit_to_page))
        .route(
            "/api/recommendations/{id}",
            patch(recommendations::patch_rec).delete(recommendations::delete_rec),
        );

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
    write_api = write_api.layer(no_store);

    let static_dir = &state.config.static_dir;
    let user_page = ServeFile::new(static_dir.join("user.html"));
    let about_page = ServeFile::new(static_dir.join("about.html"));
    let static_files = ServeDir::new(static_dir);

    let mut app = Router::new()
        .merge(read_api)
        .merge(write_api)
        .route_service("/u/{slug}", user_page)
        .route_service("/about", about_page)
        .fallback_service(static_files)
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
