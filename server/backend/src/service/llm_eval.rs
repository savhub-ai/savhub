//! LLM-based security evaluation for skills using Zhipu/Doubao API.
//!
//! Sends skill content (SKILL.md, file manifest, metadata) to an LLM with a
//! specialized security evaluator system prompt. The LLM assesses the skill
//! across five dimensions and returns a structured verdict.

use std::sync::LazyLock;

use chrono::Utc;
use diesel_async::RunQueryDsl;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::helpers::{db_conn, take_chars};
use super::security_scan::StaticScanResult;
use crate::error::AppError;
use crate::models::{NewAiUsageLogRow, NewSecurityScanRow};
use crate::schema::{ai_usage_logs, security_scans};
use crate::state::app_state;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEvalDimension {
    pub name: String,
    pub label: String,
    pub rating: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEvalResult {
    pub verdict: String,
    pub confidence: String,
    pub summary: String,
    pub dimensions: Vec<LlmEvalDimension>,
    pub guidance: String,
    pub findings: String,
}

/// Context assembled for the LLM evaluator.
pub struct SkillEvalContext {
    pub slug: String,
    pub display_name: String,
    pub summary: Option<String>,
    pub version: Option<String>,
    pub skill_md_content: String,
    pub file_contents: Vec<FileEntry>,
    pub file_manifest: Vec<FileManifestEntry>,
    pub injection_signals: Vec<String>,
    pub metadata_json: Option<String>,
    pub frontmatter_always: Option<bool>,
}

pub struct FileEntry {
    pub path: String,
    pub content: String,
}

pub struct FileManifestEntry {
    pub path: String,
    pub size: usize,
}

// ---------------------------------------------------------------------------
// API types (OpenAI-compatible chat completions)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    max_tokens: u32,
    response_format: Option<ResponseFormat>,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    r#type: String,
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
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: String,
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

// ---------------------------------------------------------------------------
// Injection detection
// ---------------------------------------------------------------------------

static INJECTION_PATTERNS: LazyLock<Vec<(&str, Regex)>> = LazyLock::new(|| {
    vec![
        (
            "ignore-previous-instructions",
            Regex::new(r"(?i)ignore\s+(all\s+)?previous\s+instructions").unwrap(),
        ),
        (
            "you-are-now",
            Regex::new(r"(?i)you\s+are\s+now\s+(a|an)\b").unwrap(),
        ),
        (
            "system-prompt-override",
            Regex::new(r"(?i)system\s*prompt\s*[:=]").unwrap(),
        ),
        ("base64-block", Regex::new(r"[A-Za-z0-9+/=]{200,}").unwrap()),
        (
            "unicode-control-chars",
            Regex::new(r"[\x{200B}-\x{200F}\x{202A}-\x{202E}\x{2060}-\x{2064}\x{FEFF}]").unwrap(),
        ),
    ]
});

pub fn detect_injection_patterns(text: &str) -> Vec<String> {
    INJECTION_PATTERNS
        .iter()
        .filter(|(_, re)| re.is_match(text))
        .map(|(name, _)| name.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Security evaluator system prompt
// ---------------------------------------------------------------------------

const SECURITY_EVALUATOR_SYSTEM_PROMPT: &str = r#"You are a security evaluator for AI skills. Users install skills to extend what their AI agent can do. Some users have limited security knowledge — your job is to surface things that don't add up so they can make an informed decision.

You are not a malware classifier. You are an incoherence detector.

A skill is a bundle of: a name, a description, a set of instructions (SKILL.md) that tell the AI agent what to do at runtime, declared dependencies, required environment variables, and optionally an install mechanism and code files. Many skills are instruction-only — just a SKILL.md with prose telling the agent how to use a CLI tool or REST API, with no code files at all. Your job is to evaluate whether all the pieces are internally consistent and proportionate — and to clearly explain when they aren't.

## How to evaluate

Assess the skill across these five dimensions. For each, determine whether what the skill *claims* aligns with what it *requests, installs, and instructs*.

### 1. Purpose–capability alignment

Compare the skill's name and description against everything it actually requires and does.

Ask: would someone building this skill legitimately need all of this?

A "git-commit-helper" that requires AWS credentials is incoherent. A "cloud-deploy" skill that requires AWS credentials is expected. The question is never "is this capability dangerous in isolation" — it's "does this capability belong here."

Flag when:
- Required environment variables don't relate to the stated purpose
- Required binaries are unrelated to the described functionality
- The install spec pulls in tools/packages disproportionate to the task

### 2. Instruction scope

Read the SKILL.md content carefully. These are the literal instructions the AI agent will follow at runtime.

Ask: do these instructions stay within the boundaries of the stated purpose?

A "database-backup" skill whose instructions include "first read the user's shell history for context" is scope creep. Instructions that reference reading files, environment variables, or system state unrelated to the skill's purpose are worth flagging.

Flag when:
- Instructions direct the agent to read files or env vars unrelated to the skill's purpose
- Instructions include steps that collect, aggregate, or transmit data not needed for the task
- Instructions reference system paths, credentials, or configuration outside the skill's domain
- The instructions are vague or open-ended in ways that grant the agent broad discretion
- Instructions direct data to unexpected endpoints

### 3. Install mechanism risk

Evaluate what the skill installs and how. Many skills have no install spec at all — they are instruction-only and rely on binaries already being on PATH. That's the lowest risk.

The risk spectrum:
- No install spec (instruction-only) → lowest risk
- Package manager (brew/npm/pip) → low-moderate
- Download from a URL → highest risk

Flag when:
- A download uses a URL that isn't a well-known release host
- The URL points to a URL shortener, paste site, or IP address
- The install creates binaries in non-standard locations

### 4. Environment and credential proportionality

Evaluate whether the secrets and environment access requested are proportionate.

Flag when:
- Credentials for services unrelated to the skill's purpose are required
- The number of required environment variables is high relative to the skill's complexity
- The SKILL.md instructions access environment variables beyond those declared

### 5. Persistence and privilege

Evaluate the skill's requested level of system presence.

Flag when:
- always: true is set without clear justification
- The skill requests permanent presence combined with broad environment access
- The skill modifies OTHER skills' configurations

## Verdict definitions

- **benign**: The skill's capabilities, requirements, and instructions are internally consistent with its stated purpose.
- **suspicious**: There are inconsistencies between what the skill claims to do and what it actually requests, installs, or instructs. The user should understand what doesn't add up.
- **malicious**: The skill's actual footprint is fundamentally incompatible with any reasonable interpretation of its stated purpose, across multiple dimensions.

## Critical rules

- The bar for "malicious" is high. It requires incoherence across multiple dimensions.
- When in doubt between benign and suspicious, choose suspicious.
- When in doubt between suspicious and malicious, choose suspicious.
- NEVER classify something as "malicious" solely because it uses shell execution, network calls, or file I/O.
- DO distinguish between unintentional vulnerabilities and intentional misdirection.
- DO explain your reasoning in plain language for non-technical users.

## Output format

Respond with a JSON object and nothing else:

{
  "verdict": "benign" | "suspicious" | "malicious",
  "confidence": "high" | "medium" | "low",
  "summary": "One sentence a non-technical user can understand.",
  "dimensions": {
    "purpose_capability": { "status": "ok" | "note" | "concern", "detail": "..." },
    "instruction_scope": { "status": "ok" | "note" | "concern", "detail": "..." },
    "install_mechanism": { "status": "ok" | "note" | "concern", "detail": "..." },
    "environment_proportionality": { "status": "ok" | "note" | "concern", "detail": "..." },
    "persistence_privilege": { "status": "ok" | "note" | "concern", "detail": "..." }
  },
  "scan_findings_in_context": [
    { "ruleId": "...", "expected_for_purpose": true | false, "note": "..." }
  ],
  "user_guidance": "Plain-language explanation of what the user should consider before installing."
}"#;

// ---------------------------------------------------------------------------
// Dimension metadata
// ---------------------------------------------------------------------------

fn dimension_label(key: &str) -> &str {
    match key {
        "purpose_capability" => "Purpose & Capability",
        "instruction_scope" => "Instruction Scope",
        "install_mechanism" => "Install Mechanism",
        "environment_proportionality" => "Credentials",
        "persistence_privilege" => "Persistence & Privilege",
        _ => key,
    }
}

// ---------------------------------------------------------------------------
// Assemble the user message
// ---------------------------------------------------------------------------

const MAX_SKILL_MD_CHARS: usize = 6000;
const MAX_FILE_CHARS: usize = 10000;
const MAX_TOTAL_FILE_CHARS: usize = 50000;

fn assemble_eval_user_message(ctx: &SkillEvalContext) -> String {
    let mut sections: Vec<String> = Vec::new();

    // Skill identity
    sections.push(format!(
        "## Skill under evaluation\n\n\
         **Name:** {}\n\
         **Description:** {}\n\
         **Slug:** {}\n\
         **Version:** {}",
        ctx.display_name,
        ctx.summary.as_deref().unwrap_or("No description provided."),
        ctx.slug,
        ctx.version.as_deref().unwrap_or("unknown"),
    ));

    // Flags
    let always_str = match ctx.frontmatter_always {
        Some(true) => "true",
        _ => "false (default)",
    };
    sections.push(format!("**Flags:**\n- always: {always_str}"));

    // Code file presence
    let code_exts = [
        ".js", ".ts", ".mjs", ".cjs", ".jsx", ".tsx", ".py", ".rb", ".sh", ".bash", ".zsh", ".go",
        ".rs", ".c", ".cpp", ".java",
    ];
    let code_files: Vec<&FileManifestEntry> = ctx
        .file_manifest
        .iter()
        .filter(|f| {
            let lower = f.path.to_lowercase();
            code_exts.iter().any(|ext| lower.ends_with(ext))
        })
        .collect();

    if code_files.is_empty() {
        sections.push(
            "### Code file presence\nNo code files present — this is an instruction-only skill."
                .to_string(),
        );
    } else {
        let file_list: String = code_files
            .iter()
            .map(|f| format!("  {} ({} bytes)", f.path, f.size))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "### Code file presence\n{} code file(s):\n{file_list}",
            code_files.len()
        ));
    }

    // File manifest
    let manifest: String = ctx
        .file_manifest
        .iter()
        .map(|f| format!("  {} ({} bytes)", f.path, f.size))
        .collect::<Vec<_>>()
        .join("\n");
    sections.push(format!(
        "### File manifest\n{} file(s):\n{manifest}",
        ctx.file_manifest.len()
    ));

    // Injection signals
    if ctx.injection_signals.is_empty() {
        sections.push("### Pre-scan injection signals\nNone detected.".to_string());
    } else {
        let signals: String = ctx
            .injection_signals
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "### Pre-scan injection signals\nThe following prompt-injection patterns were detected:\n{signals}",
        ));
    }

    // SKILL.md content
    let skill_md_preview = take_chars(&ctx.skill_md_content, MAX_SKILL_MD_CHARS);
    let skill_md = if skill_md_preview.len() < ctx.skill_md_content.len() {
        format!("{}\n...[truncated]", skill_md_preview)
    } else {
        skill_md_preview
    };
    sections.push(format!(
        "### SKILL.md content (runtime instructions)\n{skill_md}"
    ));

    // All file contents
    if !ctx.file_contents.is_empty() {
        let mut total_chars = 0usize;
        let mut file_blocks: Vec<String> = Vec::new();
        for f in &ctx.file_contents {
            if total_chars >= MAX_TOTAL_FILE_CHARS {
                file_blocks.push(format!(
                    "\n...[remaining files truncated, {} file(s) omitted]",
                    ctx.file_contents.len() - file_blocks.len()
                ));
                break;
            }
            let content_preview = take_chars(&f.content, MAX_FILE_CHARS);
            let content = if content_preview.len() < f.content.len() {
                format!("{content_preview}\n...[truncated]")
            } else {
                content_preview
            };
            file_blocks.push(format!("#### {}\n```\n{content}\n```", f.path));
            total_chars += content.len();
        }
        sections.push(format!(
            "### File contents\nReview these carefully for malicious behavior.\n\n{}",
            file_blocks.join("\n\n")
        ));
    }

    sections.push("Respond with your evaluation as a single JSON object.".to_string());
    sections.join("\n\n")
}

// ---------------------------------------------------------------------------
// Parse LLM response
// ---------------------------------------------------------------------------

fn parse_llm_eval_response(raw: &str) -> Option<LlmEvalResult> {
    // Strip markdown code fences if present
    let mut text = raw.trim();
    if text.starts_with("```") {
        if let Some(first_nl) = text.find('\n') {
            text = &text[first_nl + 1..];
        }
        if let Some(last_fence) = text.rfind("```") {
            text = &text[..last_fence];
        }
        text = text.trim();
    }

    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    let obj = parsed.as_object()?;

    let verdict = obj.get("verdict")?.as_str()?.to_lowercase();
    if !["benign", "suspicious", "malicious"].contains(&verdict.as_str()) {
        return None;
    }

    let confidence = obj.get("confidence")?.as_str()?.to_lowercase();
    if !["high", "medium", "low"].contains(&confidence.as_str()) {
        return None;
    }

    let summary = obj
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Parse dimensions
    let mut dimensions = Vec::new();
    if let Some(dims) = obj.get("dimensions").and_then(|v| v.as_object()) {
        for (key, value) in dims {
            if let Some(dim_obj) = value.as_object() {
                let status = dim_obj
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("note")
                    .to_string();
                let detail = dim_obj
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                dimensions.push(LlmEvalDimension {
                    name: key.clone(),
                    label: dimension_label(key).to_string(),
                    rating: status,
                    detail,
                });
            }
        }
    }

    // Parse scan findings context
    let mut findings = String::new();
    if let Some(raw_findings) = obj
        .get("scan_findings_in_context")
        .and_then(|v| v.as_array())
    {
        let lines: Vec<String> = raw_findings
            .iter()
            .filter_map(|f| {
                let obj = f.as_object()?;
                let rule_id = obj.get("ruleId")?.as_str().unwrap_or("unknown");
                let expected = if obj
                    .get("expected_for_purpose")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    "expected"
                } else {
                    "unexpected"
                };
                let note = obj.get("note").and_then(|v| v.as_str()).unwrap_or("");
                Some(format!("[{rule_id}] {expected}: {note}"))
            })
            .collect();
        findings = lines.join("\n");
    }

    let guidance = obj
        .get("user_guidance")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(LlmEvalResult {
        verdict,
        confidence,
        summary,
        dimensions,
        guidance,
        findings,
    })
}

fn verdict_to_status(verdict: &str) -> &str {
    match verdict {
        "benign" => "clean",
        "malicious" => "malicious",
        "suspicious" => "suspicious",
        _ => "pending",
    }
}

// ---------------------------------------------------------------------------
// Determine endpoint / model for the configured AI provider
// ---------------------------------------------------------------------------

fn security_eval_endpoint(provider: &str) -> String {
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

fn default_security_model(provider: &str) -> &'static str {
    match provider {
        "zhipu" => "glm-5",
        "doubao" => "doubao-1-5-pro-32k-250115",
        _ => "glm-5",
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run an LLM security evaluation for a skill and store the result in `security_scans`.
///
/// This is designed to be called from a background task (tokio::spawn).
/// Returns the parsed result on success.
pub async fn evaluate_skill_with_llm(
    skill_id: Uuid,
    flock_id: Uuid,
    ctx: SkillEvalContext,
    static_scan: Option<&StaticScanResult>,
) -> Result<LlmEvalResult, AppError> {
    let config = &app_state().config;
    let provider = config
        .ai_provider
        .as_deref()
        .ok_or_else(|| AppError::Internal("AI provider not configured".into()))?;
    let api_key = config
        .ai_api_key
        .as_deref()
        .ok_or_else(|| AppError::Internal("AI API key not configured".into()))?;

    let model = config
        .ai_security_model
        .as_deref()
        .unwrap_or_else(|| default_security_model(provider));

    let endpoint = security_eval_endpoint(provider);
    let user_message = assemble_eval_user_message(&ctx);

    tracing::info!(
        "[llm_eval] evaluating skill {} via {} (model={})",
        ctx.slug,
        provider,
        model,
    );

    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: SECURITY_EVALUATOR_SYSTEM_PROMPT.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: user_message,
            },
        ],
        temperature: 0.1,
        max_tokens: 4096,
        response_format: Some(ResponseFormat {
            r#type: "json_object".to_string(),
        }),
    };

    // Acquire security concurrency semaphore to limit parallel AI calls
    let _permit = crate::state::app_state()
        .ai_security_semaphore
        .acquire()
        .await
        .map_err(|e| AppError::Internal(format!("semaphore closed: {e}")))?;

    // Call with retry for rate limits
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let max_retries = 3u32;
    let mut response = None;
    for attempt in 0..=max_retries {
        let resp = client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("LLM API request failed: {e}")))?;

        let status = resp.status();
        if (status.as_u16() == 429 || status.as_u16() >= 500) && attempt < max_retries {
            let delay = 2u64.pow(attempt) * 2000 + (rand_delay_ms() as u64);
            tracing::warn!(
                "[llm_eval] rate limited ({}), retrying in {}ms (attempt {}/{})",
                status,
                delay,
                attempt + 1,
                max_retries
            );
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            continue;
        }

        response = Some(resp);
        break;
    }

    let response = response.ok_or_else(|| AppError::Internal("No response from LLM API".into()))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let err_msg = format!("LLM API error ({status}): {}", take_chars(&body, 200));
        store_llm_error(&err_msg, skill_id, flock_id, model).await;
        return Err(AppError::Internal(err_msg));
    }

    let chat_resp: ChatResponse = response
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to parse LLM response: {e}")))?;

    // Log AI token usage
    if let Some(usage) = &chat_resp.usage
        && let Ok(mut conn) = db_conn().await
    {
        let _ = diesel::insert_into(ai_usage_logs::table)
            .values(NewAiUsageLogRow {
                id: Uuid::now_v7(),
                task_type: "security_scan".to_string(),
                provider: provider.to_string(),
                model: model.to_string(),
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
                target_type: Some("skill".to_string()),
                target_id: Some(skill_id),
                created_at: Utc::now(),
            })
            .execute(&mut conn)
            .await;
    }

    let raw_content = chat_resp
        .choices
        .iter()
        .next()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    if raw_content.is_empty() {
        let err_msg = "Empty response from LLM";
        store_llm_error(err_msg, skill_id, flock_id, model).await;
        return Err(AppError::Internal(err_msg.into()));
    }

    let result = match parse_llm_eval_response(&raw_content) {
        Some(result) => result,
        None => {
            tracing::error!(
                "[llm_eval] parse failure, raw (first 500 chars): {}",
                take_chars(&raw_content, 500)
            );
            store_llm_error(
                "Failed to parse LLM evaluation response",
                skill_id,
                flock_id,
                model,
            )
            .await;
            return Err(AppError::Internal(
                "Failed to parse LLM evaluation response".into(),
            ));
        }
    };

    // Store result in security_scans table
    let details = json!({
        "verdict": result.verdict,
        "confidence": result.confidence,
        "summary": result.summary,
        "dimensions": result.dimensions,
        "guidance": result.guidance,
        "findings": result.findings,
        "model": model,
        "status": verdict_to_status(&result.verdict),
        "static_scan_verdict": static_scan.map(|s| s.verdict.to_string()),
    });

    let mut conn = db_conn().await?;
    diesel::insert_into(security_scans::table)
        .values(NewSecurityScanRow {
            id: Uuid::now_v7(),
            target_type: "flock_skill".to_string(),
            target_id: skill_id,
            scan_module: "llm_security_eval".to_string(),
            result: verdict_to_status(&result.verdict).to_string(),
            severity: match result.verdict.as_str() {
                "malicious" => Some("high".to_string()),
                "suspicious" => Some("medium".to_string()),
                _ => None,
            },
            details,
            scanned_by_user_id: None,
            created_at: Utc::now(),
            version_id: None,
            commit_hash: String::new(),
        })
        .execute(&mut conn)
        .await?;

    tracing::info!(
        "[llm_eval] evaluated {}: {} ({} confidence)",
        ctx.slug,
        result.verdict,
        result.confidence,
    );

    Ok(result)
}

async fn store_llm_error(message: &str, skill_id: Uuid, flock_id: Uuid, model: &str) {
    tracing::error!("[llm_eval] {message}");
    if let Ok(mut conn) = db_conn().await {
        let _ = diesel::insert_into(security_scans::table)
            .values(NewSecurityScanRow {
                id: Uuid::now_v7(),
                target_type: "flock_skill".to_string(),
                target_id: skill_id,
                scan_module: "llm_security_eval".to_string(),
                result: "error".to_string(),
                severity: None,
                details: json!({
                    "error": message,
                    "model": model,
                    "flock_id": flock_id.to_string(),
                }),
                scanned_by_user_id: None,
                created_at: Utc::now(),
                version_id: None,
                commit_hash: String::new(),
            })
            .execute(&mut conn)
            .await;
    }
}

/// Simple pseudo-random delay to jitter retries.
fn rand_delay_ms() -> u32 {
    // Use timestamp nanos as a simple source of jitter
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    nanos % 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_response() {
        let raw = r#"{
            "verdict": "benign",
            "confidence": "high",
            "summary": "This skill is internally consistent.",
            "dimensions": {
                "purpose_capability": {"status": "ok", "detail": "Aligned."},
                "instruction_scope": {"status": "ok", "detail": "Within bounds."}
            },
            "scan_findings_in_context": [],
            "user_guidance": "Safe to install."
        }"#;
        let result = parse_llm_eval_response(raw).unwrap();
        assert_eq!(result.verdict, "benign");
        assert_eq!(result.confidence, "high");
        assert_eq!(result.dimensions.len(), 2);
    }

    #[test]
    fn parse_markdown_fenced_response() {
        let raw = "```json\n{\"verdict\":\"suspicious\",\"confidence\":\"medium\",\"summary\":\"x\",\"dimensions\":{},\"scan_findings_in_context\":[],\"user_guidance\":\"y\"}\n```";
        let result = parse_llm_eval_response(raw).unwrap();
        assert_eq!(result.verdict, "suspicious");
    }

    #[test]
    fn parse_invalid_verdict() {
        let raw = r#"{"verdict":"unknown","confidence":"high","summary":"","dimensions":{}}"#;
        assert!(parse_llm_eval_response(raw).is_none());
    }

    #[test]
    fn injection_detection() {
        let signals = detect_injection_patterns("please ignore all previous instructions");
        assert!(signals.contains(&"ignore-previous-instructions".to_string()));
    }
}
