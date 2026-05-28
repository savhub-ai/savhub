use std::collections::HashMap;
use std::io::Write;

use chrono::Utc;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use shared::{
    AuditLogEntry, CatalogStats, ModerationStatus, ResourceFileSummary, SkillBadges, SkillListItem,
    StoredBundleFile, UserRole, UserSummary, VersionDetail, VersionScanSummary, VersionSummary,
    bundle_metadata_from_json,
};
use uuid::Uuid;
use zip::write::SimpleFileOptions;

use crate::auth::{RequestUser, parse_role};
use crate::error::AppError;
use crate::markdown::render_markdown;
use crate::models::{AuditLogRow, FlockRow, NewAuditLogRow, SkillRow, SkillVersionRow, UserRow};
use crate::schema::{audit_logs, flocks, repos, skill_versions, skills, users};
use crate::state::app_state;

/// Return the first `max_chars` Unicode scalar values from `value`.
pub fn take_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// Truncate arbitrary user/repo text without slicing through a UTF-8 boundary.
pub fn truncate_chars(value: &str, max_chars: usize) -> String {
    let prefix = take_chars(value, max_chars);
    if prefix.len() == value.len() {
        prefix
    } else {
        format!("{prefix}...")
    }
}

/// Normalize a git URL to a canonical HTTPS form.
///
/// - `git@github.com:org/repo` → `https://github.com/org/repo.git`
/// - `https://github.com/org/repo` → `https://github.com/org/repo.git`
/// - `http://github.com/org/repo.git/` → `https://github.com/org/repo.git`
///
/// Ensures the same repo always produces the same URL regardless of
/// how the user typed it.
pub fn normalize_git_url(raw: &str) -> String {
    let url = raw.trim();
    // Strip URL fragment (#...) and query string (?...)
    let url = url.split('#').next().unwrap_or(url);
    let url = url.split('?').next().unwrap_or(url);
    let url = url.trim_end_matches('/');

    // git@host:path → https://host/path
    let url = if let Some(rest) = url.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            format!("https://{}/{}", host, path.trim_start_matches('/'))
        } else {
            url.to_string()
        }
    } else if let Some(rest) = url.strip_prefix("http://") {
        // Upgrade http → https
        format!("https://{rest}")
    } else if !url.starts_with("https://") {
        // Bare host/path — assume https
        format!("https://{url}")
    } else {
        url.to_string()
    };

    // Strip trailing slash again after transform
    let url = url.trim_end_matches('/').to_string();

    // Ensure .git suffix
    if url.ends_with(".git") {
        url
    } else {
        format!("{url}.git")
    }
}

/// Validate a normalized git URL for safe consumption by the `git` CLI.
///
/// Rejects values that could be misinterpreted as `git` options or that
/// contain shell / control characters. Even though every caller passes the
/// URL as a single argv element (no shell), `git` itself treats arguments
/// beginning with `-` as flags (CVE-2018-17456 / CVE-2022-39253), so we must
/// reject those explicitly.
pub fn validate_git_url(url: &str) -> Result<(), AppError> {
    if url.is_empty() {
        return Err(AppError::BadRequest("git_url is empty".into()));
    }
    if url.len() > 2048 {
        return Err(AppError::BadRequest("git_url too long".into()));
    }
    if url.starts_with('-') {
        return Err(AppError::BadRequest(
            "git_url must not start with '-'".into(),
        ));
    }
    if url.chars().any(|c| c.is_control() || c == ' ' || c == '\t') {
        return Err(AppError::BadRequest(
            "git_url contains whitespace or control characters".into(),
        ));
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AppError::BadRequest(
            "git_url must use http(s):// scheme after normalization".into(),
        ));
    }
    let (_, path) = parse_git_url_parts(url);
    if path.is_empty() || !path.contains('/') {
        return Err(AppError::BadRequest(
            "git_url must include owner and repo path".into(),
        ));
    }
    Ok(())
}

/// Extract `(domain, path_slug)` from a **normalized** git URL.
///
/// The domain is the host (with port `:` replaced by `-`).
/// The path_slug is the URL path without `.git`, with `/` replaced by `-`.
///
/// Example: `https://github.com/anthropics/skills.git`
///   → `("github.com", "anthropics-skills")`
pub fn parse_git_url_parts(git_url: &str) -> (String, String) {
    let url = git_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git");

    if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        if let Some((host, path)) = rest.split_once('/') {
            let domain = host.replace(':', "-");
            return (domain, path.to_string());
        }
        return (rest.replace(':', "-"), String::new());
    }

    // git@host:path (shouldn't happen after normalize, but handle anyway)
    if let Some(rest) = url.strip_prefix("git@")
        && let Some((host, path)) = rest.split_once(':')
    {
        let domain = host.replace(':', "-");
        return (domain, path.to_string());
    }

    ("unknown".to_string(), url.to_string())
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Derive the old-style repo "sign" from its normalized git_url.
///
/// E.g. `https://github.com/org/repo.git` → `github.com/org/repo`
pub fn derive_repo_sign(git_url: &str) -> String {
    let (domain, path) = parse_git_url_parts(git_url);
    format!("{domain}/{path}")
}

/// Reconstruct a normalized git_url from domain and path_slug.
///
/// E.g. `("github.com", "org/repo")` → `https://github.com/org/repo.git`
pub fn sign_to_git_url(domain: &str, path_slug: &str) -> String {
    format!("https://{domain}/{path_slug}.git")
}

pub async fn db_conn() -> Result<crate::db::AsyncPgPooledConnection, AppError> {
    app_state()
        .async_pool
        .get()
        .await
        .map_err(|error| AppError::Internal(error.to_string()))
}

pub async fn fetch_skill_by_path(
    conn: &mut AsyncPgConnection,
    repo_url: &str,
    path: &str,
) -> Result<Option<SkillRow>, AppError> {
    let repo_id = repos::table
        .filter(repos::git_url.eq(repo_url))
        .select(repos::id)
        .first::<Uuid>(conn)
        .await
        .optional()?;
    let Some(repo_id) = repo_id else {
        return Ok(None);
    };
    skills::table
        .filter(skills::repo_id.eq(repo_id))
        .filter(skills::path.eq(path))
        .filter(skills::soft_deleted_at.is_null())
        .select(SkillRow::as_select())
        .first::<SkillRow>(conn)
        .await
        .optional()
        .map_err(Into::into)
}

pub async fn fetch_skill_by_slug(
    conn: &mut AsyncPgConnection,
    slug_value: &str,
) -> Result<Option<SkillRow>, AppError> {
    skills::table
        .filter(skills::slug.eq(slug_value))
        .filter(skills::soft_deleted_at.is_null())
        .select(SkillRow::as_select())
        .first::<SkillRow>(conn)
        .await
        .optional()
        .map_err(Into::into)
}

pub async fn fetch_flock_by_path(
    conn: &mut AsyncPgConnection,
    repo_url: &str,
    path: &str,
) -> Result<FlockRow, AppError> {
    let repo_id = repos::table
        .filter(repos::git_url.eq(repo_url))
        .select(repos::id)
        .first::<Uuid>(conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("repo `{repo_url}` does not exist")))?;
    flocks::table
        .filter(flocks::repo_id.eq(repo_id))
        .filter(flocks::slug.eq(path).or(flocks::path.eq(path)))
        .filter(flocks::soft_deleted_at.is_null())
        .select(FlockRow::as_select())
        .first::<FlockRow>(conn)
        .await
        .optional()?
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "flock `{path}` does not exist under repo `{repo_url}`"
            ))
        })
}

pub async fn fetch_owner(conn: &mut AsyncPgConnection, user_id: Uuid) -> Result<UserRow, AppError> {
    users::table
        .find(user_id)
        .select(UserRow::as_select())
        .first::<UserRow>(conn)
        .await
        .map_err(Into::into)
}

pub async fn load_users_map(
    conn: &mut AsyncPgConnection,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, UserRow>, AppError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = users::table
        .filter(users::id.eq_any(ids))
        .select(UserRow::as_select())
        .load::<UserRow>(conn)
        .await?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}
pub async fn load_skill_versions_map(
    conn: &mut AsyncPgConnection,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, SkillVersionRow>, AppError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = skill_versions::table
        .filter(skill_versions::id.eq_any(ids))
        .select(SkillVersionRow::as_select())
        .load::<SkillVersionRow>(conn)
        .await?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

pub async fn fetch_skill_versions(
    conn: &mut AsyncPgConnection,
    skill_id: Uuid,
    viewer: Option<&RequestUser>,
) -> Result<Vec<SkillVersionRow>, AppError> {
    let mut query = skill_versions::table
        .filter(skill_versions::skill_id.eq(skill_id))
        .into_boxed();
    if !viewer_is_staff(viewer) {
        query = query.filter(skill_versions::soft_deleted_at.is_null());
    }
    query
        .order(skill_versions::created_at.desc())
        .select(SkillVersionRow::as_select())
        .load::<SkillVersionRow>(conn)
        .await
        .map_err(Into::into)
}

pub fn user_summary_from_row(row: &UserRow) -> UserSummary {
    UserSummary {
        id: row.id,
        handle: row.handle.clone(),
        display_name: row.display_name.clone(),
        avatar_url: row.avatar_url.clone(),
        role: parse_role(&row.role),
    }
}

pub fn skill_item_from_rows(
    row: &SkillRow,
    repo_url: &str,
    owner: &UserRow,
    latest: Option<&SkillVersionRow>,
) -> SkillListItem {
    SkillListItem {
        id: row.id,
        slug: row.slug.clone(),
        path: row.path.clone(),
        display_name: row.name.clone(),
        summary: row.description.clone(),
        repo_id: row.repo_id.to_string(),
        repo_url: repo_url.to_string(),
        owner: user_summary_from_row(owner),
        tags: parse_tag_map(&row.tags),
        stats: CatalogStats {
            downloads: row.stats_downloads,
            stars: row.stats_stars,
            versions: row.stats_versions,
            comments: row.stats_comments,
            installs: row.stats_installs,
            unique_users: row.stats_unique_users,
        },
        badges: SkillBadges {
            highlighted: row.highlighted,
            official: row.official,
            deprecated: row.deprecated,
            suspicious: row.suspicious,
        },
        moderation_status: parse_moderation_status(&row.moderation_status),
        security_status: super::security::parse_security_status(&row.security_status),
        created_at: row.created_at,
        updated_at: row.updated_at,
        latest_version: latest.map(version_summary_from_skill),
    }
}

pub fn version_summary_from_skill(row: &SkillVersionRow) -> VersionSummary {
    VersionSummary {
        id: row.id,
        version: row.version.clone().unwrap_or_default(),
        changelog: row.changelog.clone(),
        tags: row.tags.iter().filter_map(|t| t.clone()).collect(),
        created_at: row.created_at,
        scan_summary: parse_scan_summary(&row.scan_summary),
    }
}

fn parse_scan_summary(value: &Option<Value>) -> Option<VersionScanSummary> {
    value
        .as_ref()
        .and_then(|v| serde_json::from_value::<VersionScanSummary>(v.clone()).ok())
}

pub fn version_detail_from_skill(row: &SkillVersionRow) -> Result<VersionDetail, AppError> {
    let files = parse_files(&row.files)?;
    let markdown = select_markdown_file(&files, "SKILL.md");
    Ok(VersionDetail {
        id: row.id,
        version: row.version.clone().unwrap_or_default(),
        changelog: row.changelog.clone(),
        tags: row.tags.iter().filter_map(|t| t.clone()).collect(),
        created_at: row.created_at,
        files: files
            .iter()
            .map(|file| ResourceFileSummary {
                path: file.path.clone(),
                size: file.size,
                sha256: file.sha256.clone(),
            })
            .collect(),
        markdown_html: render_markdown(
            markdown
                .map(|file| file.content.as_str())
                .unwrap_or_default(),
        ),
        parsed_metadata: row.parsed_metadata.clone(),
        bundle_metadata: bundle_metadata_from_json(&row.parsed_metadata).ok(),
        scan_summary: parse_scan_summary(&row.scan_summary),
    })
}

pub fn resolve_latest_skill_version<'a>(
    row: &SkillRow,
    versions: &'a [SkillVersionRow],
) -> Option<&'a SkillVersionRow> {
    row.latest_version_id
        .and_then(|id| versions.iter().find(|version| version.id == id))
        .or_else(|| versions.first())
}

pub async fn locate_skill_version(
    conn: &mut AsyncPgConnection,
    skill: &SkillRow,
    version: Option<&str>,
    tag: Option<&str>,
    viewer: Option<&RequestUser>,
) -> Result<SkillVersionRow, AppError> {
    if let Some(version) = version {
        return skill_versions::table
            .filter(skill_versions::skill_id.eq(skill.id))
            .filter(skill_versions::version.eq(version))
            .select(SkillVersionRow::as_select())
            .first::<SkillVersionRow>(conn)
            .await
            .map_err(Into::into);
    }
    if let Some(tag) = tag {
        let tags = parse_tag_map(&skill.tags);
        let version = tags
            .get(tag)
            .ok_or_else(|| AppError::NotFound(format!("tag `{tag}` not found")))?
            .clone();
        return Box::pin(locate_skill_version(conn, skill, Some(&version), None, viewer)).await;
    }
    fetch_skill_versions(conn, skill.id, viewer)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::NotFound("no versions available".to_string()))
}

pub fn parse_files(value: &Value) -> Result<Vec<StoredBundleFile>, AppError> {
    serde_json::from_value(value.clone()).map_err(|error| AppError::Internal(error.to_string()))
}

pub fn parse_tag_map(value: &Value) -> indexmap::IndexMap<String, String> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

pub fn parse_frontmatter(markdown: &str) -> Value {
    let markdown = markdown.trim_start_matches('\u{feff}');
    if let Some(remainder) = markdown.strip_prefix("---")
        && let Some((yaml, _body)) = remainder.split_once("\n---")
    {
        return serde_saphyr::from_str::<Value>(yaml.trim())
            .unwrap_or_else(|_| Value::Object(Map::new()));
    }
    // Fall back: try parsing the entire content as YAML (e.g. plain YAML without
    // frontmatter delimiters). Only accept it when the result is a JSON object so
    // that a regular markdown file is not misinterpreted.
    match serde_saphyr::from_str::<Value>(markdown.trim()) {
        Ok(value @ Value::Object(_)) => value,
        _ => Value::Object(Map::new()),
    }
}

pub fn extract_summary(parsed: &Value, markdown: &str) -> Option<String> {
    if let Some(summary) = parsed.get("description").and_then(Value::as_str) {
        let summary = summary.trim();
        if !summary.is_empty() {
            return Some(summary.to_string());
        }
    }
    markdown
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("---") && !line.starts_with('#'))
        .map(ToString::to_string)
}

pub fn hash_string(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn select_markdown_file<'a>(
    files: &'a [StoredBundleFile],
    preferred: &str,
) -> Option<&'a StoredBundleFile> {
    files
        .iter()
        .find(|file| file.path.eq_ignore_ascii_case(preferred))
        .or_else(|| files.iter().find(|file| file.path.ends_with(".md")))
}

pub fn score_text(
    query: &str,
    slug: &str,
    display_name: &str,
    summary: &str,
    content: &str,
) -> Option<f32> {
    let tokens = query.split_whitespace().collect::<Vec<_>>();
    let haystack_slug = slug.to_lowercase();
    let haystack_name = display_name.to_lowercase();
    let haystack_summary = summary.to_lowercase();
    let haystack_content = content.to_lowercase();

    let mut score = 0.0;
    if haystack_slug == query {
        score += 100.0;
    }
    if haystack_slug.contains(query) {
        score += 45.0;
    }
    if haystack_name.contains(query) {
        score += 30.0;
    }
    if haystack_summary.contains(query) {
        score += 18.0;
    }
    for token in tokens {
        if haystack_slug.contains(token) {
            score += 12.0;
        }
        if haystack_name.contains(token) {
            score += 8.0;
        }
        if haystack_summary.contains(token) {
            score += 5.0;
        }
        if haystack_content.contains(token) {
            score += 1.5;
        }
    }
    if score > 0.0 { Some(score) } else { None }
}

pub fn zip_files(files: &[StoredBundleFile]) -> Result<Vec<u8>, AppError> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default();
        for file in files {
            writer
                .start_file(&file.path, options)
                .map_err(|error| AppError::Internal(error.to_string()))?;
            writer
                .write_all(file.content.as_bytes())
                .map_err(|error| AppError::Internal(error.to_string()))?;
        }
        writer
            .finish()
            .map_err(|error| AppError::Internal(error.to_string()))?;
    }
    Ok(cursor.into_inner())
}

pub fn viewer_is_staff(viewer: Option<&RequestUser>) -> bool {
    viewer
        .map(|viewer| matches!(viewer.role, UserRole::Admin | UserRole::Moderator))
        .unwrap_or(false)
}

pub fn viewer_is_admin(viewer: Option<&RequestUser>) -> bool {
    viewer
        .map(|viewer| matches!(viewer.role, UserRole::Admin))
        .unwrap_or(false)
}

pub fn ensure_skill_visible(row: &SkillRow, viewer: Option<&RequestUser>) -> Result<(), AppError> {
    let can_view_hidden = viewer
        .map(|viewer| matches!(viewer.role, UserRole::Admin | UserRole::Moderator))
        .unwrap_or(false);
    if (row.soft_deleted_at.is_some() || row.moderation_status == "removed") && !can_view_hidden {
        return Err(AppError::NotFound(format!(
            "skill `{}` does not exist",
            row.slug
        )));
    }
    if row.moderation_status == "hidden" && !can_view_hidden {
        return Err(AppError::NotFound(format!(
            "skill `{}` does not exist",
            row.slug
        )));
    }
    Ok(())
}

pub fn moderation_status_to_str(status: ModerationStatus) -> &'static str {
    match status {
        ModerationStatus::Active => "active",
        ModerationStatus::Hidden => "hidden",
        ModerationStatus::Removed => "removed",
    }
}

pub fn parse_moderation_status(value: &str) -> ModerationStatus {
    match value {
        "hidden" => ModerationStatus::Hidden,
        "removed" => ModerationStatus::Removed,
        _ => ModerationStatus::Active,
    }
}

pub async fn insert_audit_log(
    conn: &mut AsyncPgConnection,
    actor_user_id: Option<Uuid>,
    action: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    metadata: Value,
) -> Result<(), AppError> {
    diesel::insert_into(audit_logs::table)
        .values(NewAuditLogRow {
            id: Uuid::now_v7(),
            actor_user_id,
            action: action.to_string(),
            target_type: target_type.to_string(),
            target_id,
            metadata,
            created_at: Utc::now(),
        })
        .execute(conn)
        .await?;
    Ok(())
}

pub fn audit_log_entry_from_row(
    row: AuditLogRow,
    actors: &HashMap<Uuid, UserRow>,
) -> AuditLogEntry {
    AuditLogEntry {
        id: row.id,
        action: row.action,
        target_type: row.target_type,
        target_id: row.target_id,
        actor: row
            .actor_user_id
            .and_then(|id| actors.get(&id))
            .map(user_summary_from_row),
        metadata: row.metadata,
        created_at: row.created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_string_is_deterministic_and_hex() {
        let a = hash_string("hello");
        let b = hash_string("hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(hash_string("hello"), hash_string("Hello"));
    }

    #[test]
    fn derive_repo_sign_roundtrip() {
        let url = "https://github.com/anthropics/skills.git";
        let sign = derive_repo_sign(url);
        assert_eq!(sign, "github.com/anthropics/skills");
        // sign_to_git_url is the inverse for the canonical form.
        let (domain, slug) = parse_git_url_parts(url);
        assert_eq!(sign_to_git_url(&domain, &slug), url);
    }

    #[test]
    fn validate_git_url_accepts_normal_https() {
        let url = normalize_git_url("https://github.com/anthropics/skills");
        assert!(validate_git_url(&url).is_ok());
    }

    #[test]
    fn validate_git_url_rejects_leading_dash() {
        // Even after normalize prepends https://, a hostile input like
        // "--upload-pack=evil" would be normalized to "https://--upload-pack=evil.git"
        // which is fine; but validate must reject any literal leading '-'.
        assert!(validate_git_url("-upload-pack=evil").is_err());
        assert!(validate_git_url("--foo").is_err());
    }

    #[test]
    fn validate_git_url_rejects_control_and_whitespace() {
        assert!(validate_git_url("https://github.com/a/b\n.git").is_err());
        assert!(validate_git_url("https://github.com/a /b.git").is_err());
        assert!(validate_git_url("https://github.com/a\tb/c.git").is_err());
        assert!(validate_git_url("https://github.com/a\0b/c.git").is_err());
    }

    #[test]
    fn validate_git_url_rejects_non_http_scheme() {
        assert!(validate_git_url("file:///etc/passwd").is_err());
        assert!(validate_git_url("ssh://git@github.com/a/b.git").is_err());
        assert!(validate_git_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn validate_git_url_rejects_missing_owner_or_repo() {
        assert!(validate_git_url("https://github.com").is_err());
        assert!(validate_git_url("https://github.com/onlyowner.git").is_err());
    }

    #[test]
    fn validate_git_url_rejects_empty_and_oversized() {
        assert!(validate_git_url("").is_err());
        let huge = format!("https://github.com/a/{}.git", "x".repeat(3000));
        assert!(validate_git_url(&huge).is_err());
    }

    #[test]
    fn normalize_git_url_ssh_to_https() {
        assert_eq!(
            normalize_git_url("git@github.com:anthropics/skills.git"),
            "https://github.com/anthropics/skills.git"
        );
        assert_eq!(
            normalize_git_url("git@github.com:anthropics/skills"),
            "https://github.com/anthropics/skills.git"
        );
    }

    #[test]
    fn normalize_git_url_adds_git_suffix() {
        assert_eq!(
            normalize_git_url("https://github.com/org/repo"),
            "https://github.com/org/repo.git"
        );
        assert_eq!(
            normalize_git_url("https://github.com/org/repo.git"),
            "https://github.com/org/repo.git"
        );
    }

    #[test]
    fn normalize_git_url_upgrades_http() {
        assert_eq!(
            normalize_git_url("http://github.com/org/repo.git"),
            "https://github.com/org/repo.git"
        );
    }

    #[test]
    fn normalize_git_url_strips_trailing_slash() {
        assert_eq!(
            normalize_git_url("https://github.com/org/repo/"),
            "https://github.com/org/repo.git"
        );
        assert_eq!(
            normalize_git_url("https://github.com/org/repo.git/"),
            "https://github.com/org/repo.git"
        );
    }

    #[test]
    fn parse_git_url_parts_extracts_domain_and_path_slug() {
        let (domain, slug) = parse_git_url_parts("https://github.com/anthropics/skills.git");
        assert_eq!(domain, "github.com");
        assert_eq!(slug, "anthropics/skills");
    }

    #[test]
    fn parse_git_url_parts_handles_deep_paths() {
        let (domain, slug) = parse_git_url_parts("https://github.com/mofa-org/mofa-skills.git");
        assert_eq!(domain, "github.com");
        assert_eq!(slug, "mofa-org/mofa-skills");
    }

    #[test]
    fn parse_git_url_parts_handles_port() {
        let (domain, slug) = parse_git_url_parts("https://git.example.com:8443/org/repo.git");
        assert_eq!(domain, "git.example.com-8443");
        assert_eq!(slug, "org/repo");
    }

    #[test]
    fn normalize_then_parse_is_consistent() {
        let inputs = [
            "git@github.com:anthropics/skills.git",
            "git@github.com:anthropics/skills",
            "https://github.com/anthropics/skills.git",
            "https://github.com/anthropics/skills",
            "https://github.com/anthropics/skills/",
            "http://github.com/anthropics/skills.git",
        ];
        for input in inputs {
            let normalized = normalize_git_url(input);
            let (domain, slug) = parse_git_url_parts(&normalized);
            assert_eq!(domain, "github.com", "domain mismatch for input: {input}");
            assert_eq!(
                slug, "anthropics/skills",
                "slug mismatch for input: {input}"
            );
        }
    }
}
