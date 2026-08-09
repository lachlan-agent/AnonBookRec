use anonbookrec::{config::Config, db};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let config = Config::from_env()?;
    let _pool = db::init_pool(&config.database_url).await?;
    tracing::info!("listening on {}", config.bind);
    Ok(())
}
