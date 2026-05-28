//! AI-powered metadata generation for flocks.
//!
//! When a flock contains multiple skills and has no explicit metadata,
//! we collect skill names + descriptions and ask an LLM to generate
//! a flock name and description.

use chrono::Utc;
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::helpers::{db_conn, take_chars};
use crate::error::AppError;
use crate::models::NewAiUsageLogRow;
use crate::schema::ai_usage_logs;
use crate::state::app_state;

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: i32,
    #[serde(default)]
    completion_tokens: i32,
    #[serde(default)]
    total_tokens: i32,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: String,
}

/// Generated flock metadata from AI.
#[derive(Debug, Clone, Deserialize)]
pub struct GeneratedFlockMeta {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// Generated repo metadata from AI.
#[derive(Debug, Clone, Deserialize)]
pub struct GeneratedRepoMeta {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// Determine the chat completions endpoint URL for the configured provider.
/// If `SAVHUB_AI_API_URL` is set, it is used as the base and
/// `/chat/completions` is appended.
fn chat_endpoint(provider: &str) -> String {
    let config = &crate::state::app_state().config;
    if let Some(base) = config.ai_api_url.as_deref() {
        let base = base.trim_end_matches('/');
        return format!("{base}/chat/completions");
    }
    match provider {
        "zhipu" => "https://open.bigmodel.cn/api/paas/v4/chat/completions".to_string(),
        "doubao" => "https://ark.cn-beijing.volces.com/api/v3/chat/completions".to_string(),
        _ => "https://open.bigmodel.cn/api/paas/v4/chat/completions".to_string(),
    }
}

/// Default model for the configured provider.
fn default_model(provider: &str) -> &'static str {
    match provider {
        "zhipu" => "glm-4-flash",
        "doubao" => "doubao-1-5-pro-32k-250115",
        _ => "glm-4-flash",
    }
}

/// Generate flock name and description from skill names + descriptions using AI.
///
/// Returns `None` if AI is not configured, and falls back gracefully on errors.
pub async fn generate_flock_metadata(
    skills: &[(String, String)], // (name, description)
    readme_content: Option<&str>,
) -> Option<GeneratedFlockMeta> {
    let config = &app_state().config;
    let provider = config.ai_provider.as_deref()?;
    let api_key = config.ai_api_key.as_deref()?;

    let model = config
        .ai_chat_model
        .as_deref()
        .unwrap_or_else(|| default_model(provider));

    // Build a concise summary of skills (truncate to ~2000 chars total)
    let mut skill_text = String::new();
    for (name, desc) in skills {
        let desc_short = take_chars(desc, 120);
        let line = format!("- {name}: {desc_short}\n");
        if skill_text.len() + line.len() > 2000 {
            skill_text.push_str("- ...\n");
            break;
        }
        skill_text.push_str(&line);
    }

    let readme_section = match readme_content {
        Some(text) => {
            let truncated = take_chars(text, 2000);
            format!("\nHere is the repository README for additional context:\n\n{truncated}\n")
        }
        None => String::new(),
    };

    let prompt = format!(
        r#"Given these skills in a flock (a collection of related AI skills):

{skill_text}{readme_section}
Generate a JSON object with:
- "name": a short, descriptive name for this collection (2-5 words, title case)
- "description": a one-sentence summary of what this collection provides (under 120 chars)
- "keywords": an array of 3-8 lowercase keyword strings for discoverability

Respond with ONLY the JSON object, no markdown fences."#
    );

    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are a concise metadata generator. Output only valid JSON."
                    .to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ],
        temperature: 0.3,
        max_tokens: 200,
    };

    let endpoint = chat_endpoint(provider);

    // Acquire chat concurrency semaphore
    let _permit = app_state().ai_chat_semaphore.acquire().await.ok()?;

    tracing::info!(
        "[ai] generating flock metadata via {} (model={}), {} skills",
        provider,
        model,
        skills.len()
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let response = client
        .post(endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|e| {
            tracing::warn!("[ai] request failed: {e}");
            AppError::Internal(e.to_string())
        })
        .ok()?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!("[ai] API returned {status}: {body}");
        return None;
    }

    let chat_resp: ChatResponse = response.json().await.ok()?;

    // Log AI usage
    if let Some(usage) = &chat_resp.usage
        && let Ok(mut conn) = db_conn().await
    {
        let _ = diesel::insert_into(ai_usage_logs::table)
            .values(NewAiUsageLogRow {
                id: Uuid::now_v7(),
                task_type: "flock_metadata".to_string(),
                provider: provider.to_string(),
                model: model.to_string(),
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
                target_type: Some("flock".to_string()),
                target_id: None,
                created_at: Utc::now(),
            })
            .execute(&mut conn)
            .await;
    }

    let content = chat_resp.choices.iter().next()?.message.content.clone();

    println!("[ai] raw response: {content}");

    // Parse JSON — strip markdown fences if present
    let json_str = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    match serde_json::from_str::<GeneratedFlockMeta>(json_str) {
        Ok(meta) => {
            println!(
                "[ai] generated: name={:?}, desc={:?}",
                meta.name, meta.description
            );
            Some(meta)
        }
        Err(e) => {
            tracing::warn!("[ai] failed to parse AI response as JSON: {e}, content: {json_str}");
            None
        }
    }
}

/// Generate repo name, description, and keywords from README content using AI.
pub async fn generate_repo_metadata(readme_content: &str) -> Option<GeneratedRepoMeta> {
    let config = &app_state().config;
    let provider = config.ai_provider.as_deref()?;
    let api_key = config.ai_api_key.as_deref()?;

    let model = config
        .ai_chat_model
        .as_deref()
        .unwrap_or_else(|| default_model(provider));

    let truncated = take_chars(readme_content, 4000);

    let prompt = format!(
        r#"Given this README file content:

{truncated}

Generate a JSON object with:
- "name": a short, human-friendly project name (2-5 words, title case)
- "description": a one-sentence summary of the project (under 150 chars)
- "keywords": an array of 3-8 lowercase keyword strings for discoverability

Respond with ONLY the JSON object, no markdown fences."#
    );

    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are a concise metadata generator. Output only valid JSON."
                    .to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ],
        temperature: 0.3,
        max_tokens: 300,
    };

    let endpoint = chat_endpoint(provider);

    // Acquire chat concurrency semaphore
    let _permit = app_state().ai_chat_semaphore.acquire().await.ok()?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let response = client
        .post(endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!("[ai] repo metadata API returned {status}: {body}");
        return None;
    }

    let chat_resp: ChatResponse = response.json().await.ok()?;

    if let Some(usage) = &chat_resp.usage
        && let Ok(mut conn) = db_conn().await
    {
        let _ = diesel::insert_into(ai_usage_logs::table)
            .values(NewAiUsageLogRow {
                id: Uuid::now_v7(),
                task_type: "repo_metadata".to_string(),
                provider: provider.to_string(),
                model: model.to_string(),
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
                target_type: Some("repo".to_string()),
                target_id: None,
                created_at: Utc::now(),
            })
            .execute(&mut conn)
            .await;
    }

    let content = chat_resp.choices.iter().next()?.message.content.clone();
    let json_str = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str::<GeneratedRepoMeta>(json_str).ok()
}
