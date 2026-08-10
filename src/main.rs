use std::net::SocketAddr;
use std::sync::Arc;

use anonbookrec::app::build_app;
use anonbookrec::config::Config;
use anonbookrec::resolver::{CachingResolver, OpenLibraryResolver};
use anonbookrec::state::AppState;
use anonbookrec::db;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let config = Config::from_env()?;
    let pool = db::init_pool(&config.database_url).await?;

    let resolver =
        CachingResolver::new(Arc::new(OpenLibraryResolver::new()), pool.clone());
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
