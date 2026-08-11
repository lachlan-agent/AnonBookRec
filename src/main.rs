use std::net::SocketAddr;
use std::sync::Arc;

use anonbookrec::app::build_app;
use anonbookrec::config::Config;
use anonbookrec::resolver::{
    BookResolver, CachingResolver, ChainResolver, DumpResolver, OpenLibraryResolver,
    ThrottledResolver,
};
use anonbookrec::state::AppState;
use anonbookrec::db;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let config = Config::from_env()?;
    let pool = db::init_pool(&config.database_url).await?;

    // Open Library is the last resort: one serialized, throttled client no
    // matter how many concurrent visitors, per their API guidelines.
    let open_library: Arc<dyn BookResolver> = Arc::new(ThrottledResolver::new(
        Arc::new(OpenLibraryResolver::new()),
        std::time::Duration::from_secs(1),
    ));
    // Prefer the local dump database when configured; the network fallback
    // covers books newer than (or filtered out of) the monthly dump.
    let base: Arc<dyn BookResolver> = match &config.book_db {
        Some(path) => match DumpResolver::open(path).await {
            Ok(dump) => {
                tracing::info!("book db: {}", path.display());
                Arc::new(ChainResolver::new(Arc::new(dump), open_library))
            }
            Err(err) => {
                tracing::warn!("book db {} unusable ({err}); using Open Library only", path.display());
                open_library
            }
        },
        None => open_library,
    };
    let resolver = CachingResolver::new(base, pool.clone());
    let state = AppState {
        pool,
        resolver: Arc::new(resolver),
        config: Arc::new(config.clone()),
    };
    let app = build_app(state);

    tracing::info!("listening on http://{}", config.bind);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}
