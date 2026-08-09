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
    let read_api = Router::new()
        .route("/api/pages/{slug}", get(users::get_page))
        .route("/api/global", get(recommendations::get_global));

    // Mutating routes carry the per-IP rate limit.
    let mut write_api = Router::new()
        .route("/api/users", post(users::create_page))
        .route("/api/pages/{slug}", patch(users::patch_page))
        .route("/api/pages/{slug}/recommendations", post(recommendations::submit_to_page))
        .route("/api/global/recommendations", post(recommendations::submit_global))
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
    }

    let static_dir = &state.config.static_dir;
    let user_page = ServeFile::new(static_dir.join("user.html"));
    let static_files = ServeDir::new(static_dir);

    Router::new()
        .merge(read_api)
        .merge(write_api)
        .route_service("/u/{slug}", user_page)
        .fallback_service(static_files)
        .layer(DefaultBodyLimit::max(4096))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'",
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
        ))
        .with_state(state)
}
