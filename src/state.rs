use std::sync::Arc;

use sqlx::SqlitePool;

use crate::config::Config;
use crate::resolver::BookResolver;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub resolver: Arc<dyn BookResolver>,
    pub config: Arc<Config>,
}
