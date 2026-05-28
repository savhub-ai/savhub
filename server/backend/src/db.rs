use anyhow::{Context, Result};
use diesel::Connection;
use diesel::pg::PgConnection;
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::bb8::Pool as AsyncPool;
use diesel_async::pooled_connection::bb8::PooledConnection as AsyncPooledConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

/// All runtime DB access uses this async bb8 pool over `diesel-async`.
pub type AsyncPgPool = AsyncPool<AsyncPgConnection>;

/// A connection checked out of [`AsyncPgPool`]. Connections borrow the pool,
/// which lives for the whole process inside `app_state`, hence `'static`.
pub type AsyncPgPooledConnection = AsyncPooledConnection<'static, AsyncPgConnection>;

pub const DEFAULT_DATABASE_POOL_MAX_SIZE: u32 = 32;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations");

pub fn configured_pool_max_size() -> u32 {
    std::env::var("DATABASE_POOL_MAX_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_DATABASE_POOL_MAX_SIZE)
}

/// Build the async bb8 pool used by every handler and the background worker.
pub async fn new_async_pool(database_url: &str) -> Result<AsyncPgPool> {
    let max_size = configured_pool_max_size();
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(database_url.to_string());
    AsyncPool::builder()
        .max_size(max_size)
        .connection_timeout(std::time::Duration::from_secs(5))
        .build(manager)
        .await
        .context("failed to create async PostgreSQL pool")
}

/// Run embedded migrations once at startup using a short-lived **synchronous**
/// connection. `diesel_migrations` only supports the blocking `MigrationHarness`,
/// so we establish a single connection, apply pending migrations, and drop it.
pub fn run_migrations(database_url: &str) -> Result<()> {
    let mut conn = PgConnection::establish(database_url)
        .context("failed to connect for running migrations")?;
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|error| anyhow::anyhow!("failed to run diesel migrations: {error}"))?;
    Ok(())
}
