//! Registry access backed by the Savhub REST API.
//!
//!  Registry data is fetched directly from the configured server. The only local state kept
//! here is lightweight fetched-skill metadata in JSON under `~/.savhub/`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use reqwest::blocking::{Client, Response};
use reqwest::{Method, Url};
pub use savhub_shared::{
    DataSource, RegistryFlock, RegistrySkill, RemoteSkillFetchSpec, SkillEntry,
};
use savhub_shared::{
    FlockDetailResponse, FlockSummary, ImportedSkillRecord, PagedResponse, RepoDetailResponse,
    SecurityStatus, SecuritySummary, SkillListItem,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::config::get_config_dir;
use crate::skills::{
    FetchedSkillMetadata, RepoSkillOrigin, copy_skill_folder, update_lockfile_with_metadata,
    write_repo_skill_origin,
};

const DEFAULT_API_BASE: &str = "https://savhub.ai/api/v1";
const PAGE_LIMIT: usize = 100;

pub fn read_api_base_url() -> Option<String> {
    crate::config::read_global_config()
        .ok()
        .flatten()
        .and_then(|cfg| cfg.api_base)
        .filter(|value| !value.trim().is_empty())
}

pub fn api_base_url() -> String {
    read_api_base_url().unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

fn registry_api_token() -> Option<String> {
    crate::config::read_global_config()
        .ok()
        .flatten()
        .and_then(|cfg| cfg.token)
        .filter(|value| !value.trim().is_empty())
}

#[derive(Clone)]
struct RegistryApiClient {
    base: String,
    token: Option<String>,
    client: Client,
}

impl RegistryApiClient {
    fn new() -> Result<Self> {
        Ok(Self {
            base: api_base_url(),
            token: registry_api_token(),
            client: Client::builder()
                .user_agent("savhub-local")
                .build()
                .context("failed to build registry API client")?,
        })
    }

    fn v1_url(&self, path: &str) -> Result<Url> {
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        let base = self.base.trim_end_matches('/');
        let full = if base.ends_with("/api/v1") {
            format!("{base}{path}")
        } else {
            format!("{base}/api/v1{path}")
        };
        Url::parse(&full).map_err(|error| anyhow!("invalid registry API URL: {error}"))
    }

    fn get_json_opt<T: DeserializeOwned>(&self, path: &str) -> Result<Option<T>> {
        self.get_json_url_opt(self.v1_url(path)?)
    }

    fn get_json_url<T: DeserializeOwned>(&self, url: Url) -> Result<T> {
        let response = self.send(Method::GET, url)?;
        if response.status().is_success() {
            response
                .json::<T>()
                .context("invalid registry API response payload")
        } else {
            Err(parse_api_error(response))
        }
    }

    fn get_json_url_opt<T: DeserializeOwned>(&self, url: Url) -> Result<Option<T>> {
        let response = self.send(Method::GET, url)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if response.status().is_success() {
            response
                .json::<T>()
                .map(Some)
                .context("invalid registry API response payload")
        } else {
            Err(parse_api_error(response))
        }
    }

    fn send(&self, method: Method, url: Url) -> Result<Response> {
        let mut request = self.client.request(method, url);
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }
        request.send().context("registry API request failed")
    }
}

fn parse_api_error(response: Response) -> anyhow::Error {
    let status = response.status();
    let body = response.text().unwrap_or_default();
    if let Ok(payload) = serde_json::from_str::<Value>(&body)
        && let Some(message) = payload.get("error").and_then(Value::as_str)
    {
        return anyhow!("{}: {}", status.as_u16(), message);
    }
    if body.trim().is_empty() {
        anyhow!("registry API error: {status}")
    } else {
        anyhow!("registry API error {}: {}", status.as_u16(), body.trim())
    }
}

#[derive(Debug, Clone)]
pub struct FetchedSkillInfo {
    pub slug: String,
    pub repo_sign: String,
    pub skill_path: String,
    pub local_path: PathBuf,
}

/// Search-result entry that preserves `repo_url` (unlike `RegistrySkill`),
/// so callers can fetch the skill without an extra detail round-trip.
#[derive(Debug, Clone)]
pub struct SkillSearchEntry {
    pub slug: String,
    pub path: String,
    pub name: String,
    pub description: Option<String>,
    pub repo_url: String,
    pub version: Option<String>,
}

fn repos_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("repos"))
}

fn normalize_skill_repo_path(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_matches('/')
        .to_string()
}

/// Compute the local cached path for a repo skill given its repo URL and skill path.
pub fn repo_skill_local_path(repo_url: &str, skill_path: &str) -> Option<PathBuf> {
    if repo_url.is_empty() || skill_path.is_empty() {
        return None;
    }
    let root = repos_dir().ok()?;
    Some(
        root.join(strip_git_url_scheme(repo_url))
            .join(Path::new(skill_path)),
    )
}

pub fn skill_matches_skipped(skill_ref: &str, skipped: &[String]) -> bool {
    let slug = skill_ref.rsplit('/').next().unwrap_or(skill_ref);
    skipped.iter().any(|entry| {
        entry == skill_ref
            || entry == slug
            || entry.rsplit('/').next().map(|value| value == slug) == Some(true)
    })
}

fn enum_string<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "\"unknown\"".to_string())
        .trim_matches('"')
        .to_string()
}

fn security_from_status(status: SecurityStatus) -> SecuritySummary {
    SecuritySummary {
        status: Some(enum_string(&status)),
        ..SecuritySummary::default()
    }
}

fn registry_skill_from_list_item(item: SkillListItem) -> RegistrySkill {
    RegistrySkill {
        slug: item.slug,
        path: item.path,
        name: item.display_name,
        description: item.summary,
        version: item.latest_version.map(|value| value.version),
        status: "active".to_string(),
        license: String::new(),
        categories: Vec::new(),
        keywords: Vec::new(),
        security: SecuritySummary::default(),
    }
}

fn registry_skill_from_imported(item: ImportedSkillRecord) -> RegistrySkill {
    RegistrySkill {
        slug: item.slug,
        path: item.path,
        name: item.name,
        description: item.description,
        version: item.version,
        status: enum_string(&item.status),
        license: item.license,
        categories: Vec::new(),
        keywords: Vec::new(),
        security: item.security,
    }
}

fn registry_flock_from_summary(item: FlockSummary) -> RegistryFlock {
    RegistryFlock {
        schema_version: 1,
        repo: item.repo_url,
        slug: item.slug,
        name: item.name,
        description: item.description,
        path: String::new(),
        version: item.version,
        status: enum_string(&item.status),
        visibility: item.visibility.map(|value| enum_string(&value)),
        license: item.license,
        security: security_from_status(item.security_status),
    }
}

fn registry_flock_from_detail(detail: FlockDetailResponse) -> RegistryFlock {
    RegistryFlock {
        schema_version: 1,
        repo: detail.flock.repo_url,
        slug: detail.flock.slug,
        name: detail.flock.name,
        description: detail.flock.description,
        path: detail.document.path.unwrap_or_default(),
        version: detail.flock.version,
        status: enum_string(&detail.flock.status),
        visibility: detail.flock.visibility.map(|value| enum_string(&value)),
        license: detail.flock.license,
        security: security_from_status(detail.flock.security_status),
    }
}

fn normalize_non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub fn fetch_version_label(skill_version: Option<&str>, git_sha: &str) -> String {
    if let Some(version) = skill_version
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return version.to_string();
    }
    let git_sha = git_sha.trim();
    if git_sha.is_empty() {
        "fetched".to_string()
    } else {
        git_sha.chars().take(12).collect()
    }
}

fn run_git(action: &str, cwd: Option<&Path>, args: Vec<String>) -> Result<String> {
    let mut command = Command::new("git");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.args(args.iter().map(String::as_str));
    let output = command
        .output()
        .with_context(|| format!("failed to {action}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err(anyhow!("failed to {action}"))
        } else {
            Err(anyhow!("failed to {action}: {stderr}"))
        }
    }
}

fn current_git_head(repo_root: &Path) -> Option<String> {
    normalize_non_empty(
        run_git(
            "read current git HEAD",
            Some(repo_root),
            vec!["rev-parse".to_string(), "HEAD".to_string()],
        )
        .ok(),
    )
}

fn current_remote_url(repo_root: &Path) -> Option<String> {
    normalize_non_empty(
        run_git(
            "read current git remote URL",
            Some(repo_root),
            vec![
                "config".to_string(),
                "--get".to_string(),
                "remote.origin.url".to_string(),
            ],
        )
        .ok(),
    )
}

fn repo_checkout_dir(repo_sign: &str) -> Result<PathBuf> {
    let dir_name = strip_git_url_scheme(repo_sign);
    Ok(repos_dir()?.join(Path::new(&dir_name)))
}

/// Strip `https://`/`http://` prefix and `.git` suffix for use as a filesystem path.
fn strip_git_url_scheme(url: &str) -> String {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .to_string()
}

pub fn ensure_repo_checkout(repo_sign: &str, git_url: &str, git_sha: &str) -> Result<PathBuf> {
    let repo_root = repo_checkout_dir(repo_sign)?;
    let git_dir = repo_root.join(".git");

    if git_dir.is_dir() {
        let remote_url = current_remote_url(&repo_root);
        if remote_url.as_deref() != Some(git_url) {
            fs::remove_dir_all(&repo_root).with_context(|| {
                format!(
                    "failed to remove stale repo checkout at {}",
                    repo_root.display()
                )
            })?;
        }
    } else if repo_root.exists() {
        fs::remove_dir_all(&repo_root)
            .with_context(|| format!("failed to remove {}", repo_root.display()))?;
    }

    if !repo_root.join(".git").is_dir() {
        if let Some(parent) = repo_root.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        run_git(
            "clone remote repo",
            None,
            vec![
                "clone".to_string(),
                git_url.to_string(),
                repo_root.display().to_string(),
            ],
        )?;
    } else if current_git_head(&repo_root).as_deref() == Some(git_sha) {
        return Ok(repo_root);
    } else {
        run_git(
            "fetch remote repo",
            Some(&repo_root),
            vec![
                "fetch".to_string(),
                "--tags".to_string(),
                "--force".to_string(),
                "--prune".to_string(),
                "origin".to_string(),
            ],
        )?;
    }

    run_git(
        "checkout repo revision",
        Some(&repo_root),
        vec![
            "checkout".to_string(),
            "--force".to_string(),
            "--detach".to_string(),
            git_sha.to_string(),
        ],
    )?;

    let current_head = current_git_head(&repo_root).unwrap_or_default();
    if current_head != git_sha {
        return Err(anyhow!(
            "repo `{repo_sign}` checked out `{current_head}` instead of `{git_sha}`"
        ));
    }

    Ok(repo_root)
}

pub fn cache_remote_skill_from_repo(spec: &RemoteSkillFetchSpec) -> Result<PathBuf> {
    let repo_root = ensure_repo_checkout(&spec.repo_sign, &spec.git_url, &spec.git_sha)?;
    let skill_path = normalize_skill_repo_path(&spec.skill_path);
    if skill_path.is_empty() {
        return Err(anyhow!("skill path is empty for repo `{}`", spec.repo_sign));
    }
    let skill_root = repo_root.join(Path::new(&skill_path));
    let metadata = fs::metadata(&skill_root).with_context(|| {
        format!(
            "skill path `{}` was not found in repo `{}` at `{}`",
            spec.skill_path, spec.repo_sign, spec.git_sha
        )
    })?;
    if !metadata.is_dir() {
        return Err(anyhow!(
            "skill path `{}` is not a directory in repo `{}`",
            spec.skill_path,
            spec.repo_sign
        ));
    }
    Ok(skill_root)
}

pub fn install_remote_skill_from_repo(spec: &RemoteSkillFetchSpec, target: &Path) -> Result<()> {
    let skill_root = cache_remote_skill_from_repo(spec)?;
    copy_skill_folder(&skill_root, target)
        .with_context(|| format!("failed to install skill into {}", target.display()))
}

fn fetch_skill_page(
    client: &RegistryApiClient,
    query: Option<&str>,
    page: usize,
    page_size: usize,
) -> Result<(Vec<SkillListItem>, bool)> {
    let mut url = client.v1_url("/skills")?;
    url.query_pairs_mut()
        .append_pair("limit", &page_size.to_string())
        .append_pair("sort", "updated")
        .append_pair("cursor", &(page.saturating_mul(page_size)).to_string());
    if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
        url.query_pairs_mut().append_pair("q", query.trim());
    }
    let response = client.get_json_url::<PagedResponse<SkillListItem>>(url)?;
    Ok((response.items, response.next_cursor.is_some()))
}

fn fetch_flock_page(
    client: &RegistryApiClient,
    query: Option<&str>,
    page: usize,
    page_size: usize,
) -> Result<(Vec<FlockSummary>, bool)> {
    let mut url = client.v1_url("/flocks")?;
    url.query_pairs_mut()
        .append_pair("limit", &page_size.to_string())
        .append_pair("sort", "updated")
        .append_pair("cursor", &(page.saturating_mul(page_size)).to_string());
    if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
        url.query_pairs_mut().append_pair("q", query.trim());
    }
    let response = client.get_json_url::<PagedResponse<FlockSummary>>(url)?;
    Ok((response.items, response.next_cursor.is_some()))
}

fn estimate_total(page: usize, page_size: usize, len: usize, has_more: bool) -> usize {
    let seen = page.saturating_mul(page_size).saturating_add(len);
    if has_more {
        seen.saturating_add(1)
    } else {
        seen
    }
}

fn remote_repo_detail(repo_url: &str) -> Result<Option<RepoDetailResponse>> {
    let route_path = git_url_to_route_path(repo_url);
    RegistryApiClient::new()?.get_json_opt(&format!("/repos/{route_path}"))
}

/// Convert a git URL to the API route path (strip scheme and .git suffix).
fn git_url_to_route_path(url: &str) -> String {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .to_string()
}

fn remote_flock_detail_by_id(id: &str) -> Result<Option<FlockDetailResponse>> {
    RegistryApiClient::new()?.get_json_opt(&format!("/flocks/{id}"))
}

pub fn list_skills(
    query: Option<&str>,
    _status_filter: Option<&str>,
    page: usize,
    page_size: usize,
) -> Result<(Vec<RegistrySkill>, usize)> {
    let client = RegistryApiClient::new()?;
    let (items, has_more) = fetch_skill_page(&client, query, page, page_size)?;
    let mapped = items
        .into_iter()
        .map(registry_skill_from_list_item)
        .collect::<Vec<_>>();
    let total = estimate_total(page, page_size, mapped.len(), has_more);
    Ok((mapped, total))
}

pub fn search_skills(query: &str, limit: usize) -> Result<Vec<RegistrySkill>> {
    let (skills, _) = list_skills(Some(query), None, 0, limit)?;
    Ok(skills)
}

/// Search registry skills, preserving `repo_url` and `path` so callers can
/// fetch a chosen entry without a follow-up detail request.
pub fn search_skill_entries(query: &str, limit: usize) -> Result<Vec<SkillSearchEntry>> {
    let client = RegistryApiClient::new()?;
    let query = if query.trim().is_empty() {
        None
    } else {
        Some(query)
    };
    let (items, _) = fetch_skill_page(&client, query, 0, limit)?;
    Ok(items
        .into_iter()
        .map(|item| SkillSearchEntry {
            slug: item.slug,
            path: item.path,
            name: item.display_name,
            description: item.summary,
            repo_url: item.repo_url,
            version: item.latest_version.map(|value| value.version),
        })
        .collect())
}

/// Search registry flocks by query (server-side filter).
pub fn search_flocks(query: &str, limit: usize) -> Result<Vec<RegistryFlock>> {
    let client = RegistryApiClient::new()?;
    let query = if query.trim().is_empty() {
        None
    } else {
        Some(query)
    };
    let (items, _) = fetch_flock_page(&client, query, 0, limit)?;
    Ok(items.into_iter().map(registry_flock_from_summary).collect())
}

pub fn list_flocks() -> Result<Vec<RegistryFlock>> {
    let client = RegistryApiClient::new()?;
    let mut page = 0usize;
    let mut all = Vec::new();
    loop {
        let (items, has_more) = fetch_flock_page(&client, None, page, PAGE_LIMIT)?;
        if items.is_empty() {
            return Ok(all);
        }
        all.extend(items.into_iter().map(registry_flock_from_summary));
        if !has_more {
            return Ok(all);
        }
        page += 1;
    }
}

pub fn get_flock_by_slug(repo_url: &str, path: &str) -> Result<Option<RegistryFlock>> {
    let Some(repo) = remote_repo_detail(repo_url)? else {
        return Ok(None);
    };
    let Some(summary) = repo.flocks.into_iter().find(|f| f.slug == path) else {
        return Ok(None);
    };
    if let Some(detail) = remote_flock_detail_by_id(&summary.id.to_string())? {
        return Ok(Some(registry_flock_from_detail(detail)));
    }
    Ok(Some(registry_flock_from_summary(summary)))
}

pub fn list_skills_in_flock(repo_url: &str, path: &str) -> Result<Vec<RegistrySkill>> {
    let Some(repo) = remote_repo_detail(repo_url)? else {
        return Ok(Vec::new());
    };
    let Some(summary) = repo.flocks.into_iter().find(|f| f.slug == path) else {
        return Ok(Vec::new());
    };
    let Some(detail) = remote_flock_detail_by_id(&summary.id.to_string())? else {
        return Ok(Vec::new());
    };
    Ok(detail
        .skills
        .into_iter()
        .map(registry_skill_from_imported)
        .collect())
}

/// List skill paths in the given flock, without fetching full skill details.
pub fn list_flock_skills(repo_url: &str, path: &str) -> Result<Vec<String>> {
    Ok(list_skills_in_flock(repo_url, path)?
        .into_iter()
        .map(|skill| skill.slug)
        .collect())
}

pub fn list_repo_flock_refs(repo_url: &str) -> Result<Vec<crate::selectors::SelectorSkillRef>> {
    let Some(detail) = remote_repo_detail(repo_url)? else {
        return Ok(Vec::new());
    };
    Ok(detail
        .flocks
        .into_iter()
        .map(|flock| crate::selectors::SelectorSkillRef {
            repo: flock.repo_url,
            path: flock.slug,
        })
        .collect())
}

pub fn fetch_skills_batch(repo_paths: &[(String, String)]) -> Result<Vec<FetchedSkillInfo>> {
    fetch_skills_batch_with_progress(repo_paths, |_, _, _| {})
}

/// Fetch skills in batch with a per-skill progress callback.
///
/// Groups skills by repo so each repo is fetched from the API and cloned only
/// once, then all skills in that repo are resolved from the local checkout.
///
/// `on_progress(index, total, result)` is called after each skill is processed.
/// `result` is `Ok(slug)` on success or `Err(message)` on failure.
pub fn fetch_skills_batch_with_progress(
    repo_paths: &[(String, String)],
    mut on_progress: impl FnMut(usize, usize, Result<&str, &str>),
) -> Result<Vec<FetchedSkillInfo>> {
    // Deduplicate
    let mut seen = BTreeSet::new();
    let mut requested: Vec<(String, String)> = Vec::new();
    for (repo_url, skill_path) in repo_paths {
        let repo_url = repo_url.trim().to_string();
        let skill_path = skill_path.trim().to_string();
        let key = format!("{repo_url}/{skill_path}");
        if !key.is_empty() && seen.insert(key) {
            requested.push((repo_url, skill_path));
        }
    }

    // Group by repo_url, preserving insertion order
    let mut repo_groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut repo_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (repo_url, skill_path) in &requested {
        if let Some(&idx) = repo_index.get(repo_url) {
            repo_groups[idx].1.push(skill_path.clone());
        } else {
            repo_index.insert(repo_url.clone(), repo_groups.len());
            repo_groups.push((repo_url.clone(), vec![skill_path.clone()]));
        }
    }

    let config_dir = get_config_dir()?;
    let security_level = crate::config::read_global_config()
        .ok()
        .flatten()
        .map(|c| c.security_level)
        .unwrap_or_default();
    let mut results = Vec::new();
    let total = requested.len();
    let mut progress_idx = 0usize;

    for (repo_url, skill_paths) in &repo_groups {
        // 1) Fetch repo detail from API — ONCE per repo
        let repo = match remote_repo_detail(repo_url)? {
            Some(value) => value,
            None => {
                for sp in skill_paths {
                    on_progress(progress_idx, total, Err(sp));
                    progress_idx += 1;
                }
                continue;
            }
        };

        let repo_sign = git_url_to_route_path(&repo.document.git_url);
        let git_sha = match normalize_non_empty(repo.document.git_sha.clone()) {
            Some(sha) => sha,
            None => {
                for sp in skill_paths {
                    on_progress(progress_idx, total, Err(sp));
                    progress_idx += 1;
                }
                continue;
            }
        };

        // 2) Ensure repo checkout — ONCE per repo (clone or fetch+checkout)
        let repo_root = match ensure_repo_checkout(&repo_sign, &repo.document.git_url, &git_sha) {
            Ok(root) => root,
            Err(_) => {
                for sp in skill_paths {
                    on_progress(progress_idx, total, Err(sp));
                    progress_idx += 1;
                }
                continue;
            }
        };

        // Build a lookup from the repo detail skills list (no extra API calls)
        let skill_lookup: std::collections::HashMap<&str, &savhub_shared::ImportedSkillRecord> =
            repo.skills
                .iter()
                .flat_map(|s| {
                    let mut entries = vec![(s.slug.as_str(), s)];
                    entries.push((s.path.as_str(), s));
                    entries
                })
                .collect();

        // 3) For each skill: just resolve path from local checkout + copy
        for skill_path in skill_paths {
            let record = skill_lookup.get(skill_path.as_str());

            // Enforce security level: skip skills that don't pass the check
            if let Some(r) = record {
                let status = r.security.status.as_deref();
                let verdict = r.security.verdict.as_deref();
                if !security_level.allows(status, verdict) {
                    on_progress(progress_idx, total, Err(skill_path));
                    progress_idx += 1;
                    continue;
                }
            }

            let (resolved_path, skill_version) = match record {
                Some(r) => (r.path.clone(), normalize_non_empty(r.version.clone())),
                None => {
                    // Skill not found in repo's skill list — try using skill_path directly
                    (skill_path.clone(), None)
                }
            };

            let normalized = normalize_skill_repo_path(&resolved_path);
            let local_path = repo_root.join(Path::new(&normalized));

            if !local_path.is_dir() {
                on_progress(progress_idx, total, Err(skill_path));
                progress_idx += 1;
                continue;
            }

            // Use the canonical slug from the registry record if available,
            // otherwise extract the last path component (e.g. "skills/foo" → "foo").
            let slug = match record {
                Some(r) => r.slug.clone(),
                None => skill_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(skill_path)
                    .to_string(),
            };
            let _ = write_repo_skill_origin(
                &local_path,
                &RepoSkillOrigin {
                    version: 1,
                    repo: api_base_url(),
                    repo_sign: repo_sign.clone(),
                    repo_commit: Some(git_sha.clone()),
                    slug: slug.clone(),
                    skill_version: skill_version.clone(),
                    fetched_at: Utc::now().timestamp_millis(),
                },
            );

            let version = skill_version.as_deref().unwrap_or(&git_sha);
            update_lockfile_with_metadata(
                &config_dir,
                &slug,
                version,
                &FetchedSkillMetadata {
                    remote_slug: Some(slug.clone()),
                    repo_url: Some(repo_sign.clone()),
                    path: Some(normalized.clone()),
                    flock_slug: None,
                    git_sha: Some(git_sha.clone()),
                },
            );

            on_progress(progress_idx, total, Ok(&slug));
            progress_idx += 1;

            results.push(FetchedSkillInfo {
                slug,
                repo_sign: repo_sign.clone(),
                skill_path: resolved_path,
                local_path,
            });
        }
    }

    Ok(results)
}
