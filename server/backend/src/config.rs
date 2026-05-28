use std::{env, fmt};

use anyhow::{Context, Result};

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind: String,
    pub frontend_origin: String,
    pub api_base: String,
    pub space_path: String,
    pub github_client_id: String,
    pub github_client_secret: String,
    pub github_redirect_url: String,
    pub github_admin_logins: Vec<String>,
    pub github_moderator_logins: Vec<String>,
    pub sync_interval_secs: u64,
    pub sync_stale_hours: u64,
    /// AI provider for generating flock metadata. "zhipu" or "doubao".
    pub ai_provider: Option<String>,
    /// API key for the configured AI provider.
    pub ai_api_key: Option<String>,
    /// Custom API base URL (overrides provider default endpoint).
    pub ai_api_url: Option<String>,
    /// Model name override for chat completions.
    pub ai_chat_model: Option<String>,
    /// Model name override for LLM security evaluation (defaults to glm-4-plus /
    /// doubao-1-5-pro-32k).
    pub ai_security_model: Option<String>,
    pub auto_index_min_interval_secs: u64,
    /// Maximum number of index jobs that may execute in parallel. Default 3.
    pub max_parallel_index_jobs: usize,
    /// Enable the enhanced security scanning pipeline (LLM). Default false.
    pub ai_security_scan_enabled: bool,
    /// Max concurrent AI chat requests (flock/repo metadata). Default 2.
    pub ai_chat_concurrency: usize,
    /// Max concurrent AI security scan requests. Default 2.
    pub ai_security_concurrency: usize,
    /// Number of static security scan worker threads. Default 1.
    pub static_scan_concurrency: usize,
}

impl Config {
    pub fn repo_checkout_base_path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.space_path).join("repos")
    }
}

fn redact(value: &str) -> &'static str {
    if value.is_empty() {
        "<unset>"
    } else {
        "<redacted>"
    }
}

fn redact_opt(value: Option<&String>) -> &'static str {
    match value {
        Some(v) if !v.is_empty() => "<redacted>",
        _ => "<unset>",
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &redact(&self.database_url))
            .field("bind", &self.bind)
            .field("frontend_origin", &self.frontend_origin)
            .field("api_base", &self.api_base)
            .field("space_path", &self.space_path)
            .field("github_client_id", &self.github_client_id)
            .field("github_client_secret", &redact(&self.github_client_secret))
            .field("github_redirect_url", &self.github_redirect_url)
            .field("github_admin_logins", &self.github_admin_logins)
            .field("github_moderator_logins", &self.github_moderator_logins)
            .field("sync_interval_secs", &self.sync_interval_secs)
            .field("sync_stale_hours", &self.sync_stale_hours)
            .field("ai_provider", &self.ai_provider)
            .field("ai_api_key", &redact_opt(self.ai_api_key.as_ref()))
            .field("ai_api_url", &self.ai_api_url)
            .field("ai_chat_model", &self.ai_chat_model)
            .field("ai_security_model", &self.ai_security_model)
            .field(
                "auto_index_min_interval_secs",
                &self.auto_index_min_interval_secs,
            )
            .field("max_parallel_index_jobs", &self.max_parallel_index_jobs)
            .field("ai_security_scan_enabled", &self.ai_security_scan_enabled)
            .field("ai_chat_concurrency", &self.ai_chat_concurrency)
            .field("ai_security_concurrency", &self.ai_security_concurrency)
            .field("static_scan_concurrency", &self.static_scan_concurrency)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // Use .env.local if it exists, otherwise fall back to .env
        if dotenvy::from_filename(".env.local").is_err() {
            dotenvy::dotenv().ok();
        }
        let database_url =
            env::var("DATABASE_URL").context("DATABASE_URL is required to start the backend")?;
        let bind = env::var("SAVHUB_BIND").unwrap_or_else(|_| "127.0.0.1:5006".to_string());
        let frontend_origin = env::var("SAVHUB_FRONTEND_ORIGIN")
            .unwrap_or_else(|_| "http://127.0.0.1:8081".to_string());
        let api_base =
            env::var("SAVHUB_API_BASE").unwrap_or_else(|_| format!("http://{bind}/api/v1"));
        let space_path = env::var("SAVHUB_SPACE_PATH").unwrap_or_else(|_| "./space".to_string());
        let github_client_id = env::var("SAVHUB_GITHUB_CLIENT_ID")
            .context("SAVHUB_GITHUB_CLIENT_ID is required to start the backend")?;
        let github_client_secret = env::var("SAVHUB_GITHUB_CLIENT_SECRET")
            .context("SAVHUB_GITHUB_CLIENT_SECRET is required to start the backend")?;
        let github_redirect_url = env::var("SAVHUB_GITHUB_REDIRECT_URL")
            .context("SAVHUB_GITHUB_REDIRECT_URL is required to start the backend")?;
        Ok(Self {
            database_url,
            bind,
            frontend_origin,
            api_base,
            space_path,
            github_client_id,
            github_client_secret,
            github_redirect_url,
            github_admin_logins: parse_login_list("SAVHUB_GITHUB_ADMIN_LOGINS"),
            github_moderator_logins: parse_login_list("SAVHUB_GITHUB_MODERATOR_LOGINS"),
            sync_interval_secs: env::var("SAVHUB_SYNC_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(900),
            sync_stale_hours: env::var("SAVHUB_SYNC_STALE_HOURS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(6),
            ai_provider: env::var("SAVHUB_AI_PROVIDER")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            ai_api_key: env::var("SAVHUB_AI_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            ai_api_url: env::var("SAVHUB_AI_API_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            ai_chat_model: env::var("SAVHUB_AI_CHAT_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            ai_security_model: env::var("SAVHUB_AI_SECURITY_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            auto_index_min_interval_secs: env::var("SAVHUB_AUTO_INDEX_MIN_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
            max_parallel_index_jobs: env::var("SAVHUB_MAX_PARALLEL_INDEX_JOBS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            ai_security_scan_enabled: env::var("SAVHUB_AI_SECURITY_SCAN")
                .map(|v| matches!(v.trim().to_lowercase().as_str(), "true" | "1" | "yes"))
                .unwrap_or(false),
            ai_chat_concurrency: env::var("SAVHUB_AI_CHAT_CONCURRENCY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),
            ai_security_concurrency: env::var("SAVHUB_AI_SECURITY_CONCURRENCY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),
            static_scan_concurrency: env::var("SAVHUB_SECURITY_STATIC_SCAN_CONCURRENCY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
        })
    }
}

fn parse_login_list(name: &str) -> Vec<String> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(|item| item.trim().to_lowercase())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}
