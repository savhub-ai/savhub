use anyhow::Result;
use salvo::cors::{Any, Cors};
use salvo::http::Method;
use salvo::prelude::*;
use server::config::Config;
use server::db::{configured_pool_max_size, new_async_pool, run_migrations};
use server::seed::ensure_seed_data;
use server::state::init_state;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,backend=debug,sqlx=warn".to_string()),
        )
        .init();

    let config = Config::from_env()?;

    // ── startup diagnostics ──
    tracing::info!("savhub backend starting up");
    tracing::info!("  bind            = {}", config.bind);
    tracing::info!("  frontend_origin = {}", config.frontend_origin);
    tracing::info!("  api_base        = {}", config.api_base);
    tracing::info!("  space_path      = {}", config.space_path);

    // check git is available
    match std::process::Command::new("git").arg("--version").output() {
        Ok(out) => tracing::info!(
            "  git             = {}",
            String::from_utf8_lossy(&out.stdout).trim()
        ),
        Err(e) => tracing::error!("  git             = NOT FOUND: {e}"),
    }

    let db_pool_max_size = configured_pool_max_size();
    run_migrations(&config.database_url)?;
    tracing::info!("  database        = connected, migrations applied");
    tracing::info!("  db_pool_max_size = {}", db_pool_max_size);
    tracing::info!(
        "  index_job_concurrency = {}",
        config.max_parallel_index_jobs
    );
    tracing::info!(
        "  static_scan_concurrency = {}",
        config.static_scan_concurrency
    );
    // Security & AI diagnostics
    if config.ai_security_scan_enabled {
        tracing::info!("  ai_security_scan = enabled");
    } else {
        tracing::warn!(
            "  ai_security_scan = DISABLED (set SAVHUB_AI_SECURITY_SCAN=true to enable)"
        );
    }
    match (&config.ai_provider, &config.ai_api_key) {
        (Some(provider), Some(_)) => {
            tracing::info!("  ai_provider     = {provider}");
            tracing::info!(
                "  ai_api_url      = {}",
                config.ai_api_url.as_deref().unwrap_or("(provider default)")
            );
            tracing::info!(
                "  ai_chat_model   = {}",
                config.ai_chat_model.as_deref().unwrap_or("(default)")
            );
            tracing::info!(
                "  ai_security_model = {}",
                config.ai_security_model.as_deref().unwrap_or("(default)")
            );
            tracing::info!("  ai_chat_concurrency = {}", config.ai_chat_concurrency);
            tracing::info!(
                "  ai_security_concurrency = {}",
                config.ai_security_concurrency
            );
        }
        _ => {
            tracing::warn!(
                "  ai_provider     = NOT CONFIGURED (set SAVHUB_AI_PROVIDER and SAVHUB_AI_API_KEY)"
            );
        }
    }

    if config.max_parallel_index_jobs >= db_pool_max_size as usize {
        tracing::warn!(
            "  index_job_concurrency >= db_pool_max_size; concurrent indexing may contend with API traffic"
        );
    }
    if config.static_scan_concurrency >= db_pool_max_size as usize {
        tracing::warn!(
            "  static_scan_concurrency >= db_pool_max_size; tune this if static scans pressure the pool"
        );
    }

    let async_pool = new_async_pool(&config.database_url).await?;
    tracing::info!("  async_db_pool   = ready");
    let _state = init_state(config.clone(), async_pool.clone())?;
    // Backfill repos.git_sha for any rows that are still NULL.
    {
        let pool = async_pool.clone();
        tokio::spawn(async move {
            if let Err(e) = server::service::upgrade::backfill_repo_git_sha(&pool).await {
                tracing::error!("backfill_repo_git_sha failed: {e}");
            }
        });
    }
    ensure_seed_data(&async_pool).await?;
    let cors = Cors::new()
        .allow_origin(config.frontend_origin.as_str())
        .allow_methods(vec![
            Method::GET,
            Method::POST,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
        .into_handler();

    let _worker = server::worker::spawn_worker(async_pool.clone());

    let acceptor = TcpListener::new(config.bind.clone()).bind().await;
    tracing::info!("savhub backend listening on {}", config.bind);
    Server::new(acceptor)
        .serve(server::routing::router().hoop(cors))
        .await;
    Ok(())
}
