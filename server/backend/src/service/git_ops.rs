use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use uuid::Uuid;

use super::helpers::{extract_summary, parse_frontmatter};
use crate::error::AppError;
use crate::state::app_state;

/// On Windows, configure the repo so that files with characters illegal in
/// Windows paths (e.g. `:`) are accepted in the index but excluded from the
/// worktree. Two settings work together:
///
/// - `core.protectNTFS = false` lets git keep the entry in the index.
/// - sparse-checkout marks those entries as skip-worktree so git never writes them to disk.
///
/// No-op on non-Windows. Idempotent.
async fn setup_windows_sparse_checkout(repo_dir: &Path) {
    if !cfg!(windows) {
        return;
    }

    let _ = tokio::process::Command::new("git")
        .args(["config", "core.protectNTFS", "false"])
        .current_dir(repo_dir)
        .output()
        .await;
    let _ = tokio::process::Command::new("git")
        .args(["config", "core.sparseCheckout", "true"])
        .current_dir(repo_dir)
        .output()
        .await;

    let info_dir = repo_dir.join(".git").join("info");
    let _ = fs::create_dir_all(&info_dir);
    let _ = fs::write(info_dir.join("sparse-checkout"), "*\n!*:*\n");
}

#[derive(Debug, Clone)]
pub(crate) struct SkillCandidate {
    pub(crate) path: PathBuf,
    pub(crate) relative_dir: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ScannedSkillMetadata {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) version: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CachedRepoCheckout {
    pub(crate) path: PathBuf,
    pub(crate) head_sha: String,
    pub(crate) previous_sha: Option<String>,
    pub(crate) changed_skill_files: Vec<String>,
    pub(crate) reused: bool,
    /// If git followed an HTTP redirect (e.g. the repo was moved/renamed),
    /// this contains the new URL so callers can update their records.
    pub(crate) redirected_url: Option<String>,
}

/// When a git remote returns an HTTP redirect (301/302), the repo has been
/// moved or renamed. This function updates the database (repos, flocks,
/// skills) so everything points to the new URL.
///
/// `old_url` and `new_url` should both be normalized HTTPS URLs.
pub(crate) async fn apply_repo_redirect(
    conn: &mut diesel_async::AsyncPgConnection,
    repo_id: Uuid,
    old_url: &str,
    new_url: &str,
) -> Result<(), AppError> {
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;

    use crate::models::RepoChangeset;
    use crate::schema::repos;

    let new_url = super::helpers::normalize_git_url(new_url);
    let (new_domain, new_path_slug) = super::helpers::parse_git_url_parts(&new_url);
    let new_sign = format!("{new_domain}/{new_path_slug}");

    let old_url_normalized = super::helpers::normalize_git_url(old_url);
    let (old_domain, old_path_slug) = super::helpers::parse_git_url_parts(&old_url_normalized);
    let old_sign = format!("{old_domain}/{old_path_slug}");

    if old_sign == new_sign {
        return Ok(());
    }

    tracing::info!(
        repo_id = %repo_id,
        old_sign = old_sign.as_str(),
        new_sign = new_sign.as_str(),
        "applying repo redirect: updating DB"
    );

    diesel::update(repos::table.find(repo_id))
        .set(RepoChangeset {
            git_url: Some(new_url.clone()),
            updated_at: Some(chrono::Utc::now()),
            ..Default::default()
        })
        .execute(conn)
        .await
        .map_err(|error| {
            AppError::Internal(format!("failed to update repo URL after redirect: {error}"))
        })?;

    Ok(())
}

pub(crate) fn collect_skill_candidates(
    root: &Path,
    relative_dir: &str,
    out: &mut Vec<SkillCandidate>,
) -> Result<(), AppError> {
    let entries = fs::read_dir(root)
        .map_err(|error| AppError::Internal(format!("failed to walk checked out repo: {error}")))?;
    let mut subdirs = Vec::new();
    let mut has_skill_md = false;

    for entry in entries {
        let entry = entry.map_err(|error| {
            AppError::Internal(format!("failed to walk checked out repo: {error}"))
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = entry.metadata().map_err(|error| {
            AppError::Internal(format!("failed to inspect checked out repo: {error}"))
        })?;
        if metadata.is_dir() {
            if should_skip_dir(&name) {
                continue;
            }
            subdirs.push((entry.path(), join_relative_dir(relative_dir, &name)));
            continue;
        }

        if metadata.is_file() && name == "SKILL.md" {
            has_skill_md = true;
        }
    }

    // Always recurse into subdirectories first to find nested skills.
    let before = out.len();
    for (path, relative) in subdirs {
        collect_skill_candidates(&path, &relative, out)?;
    }
    let found_children = out.len() > before;

    // Only register this directory as a skill candidate if it has a SKILL.md
    // and no child directories contained their own SKILL.md.  This prevents a
    // root-level SKILL.md from swallowing an entire repo of sub-skills (e.g.
    // repos where each subdirectory is a standalone skill).
    if has_skill_md && !found_children {
        out.push(SkillCandidate {
            path: root.to_path_buf(),
            relative_dir: relative_dir.to_string(),
        });
    }
    Ok(())
}

fn should_skip_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | ".git" | ".savhub")
}

fn join_relative_dir(base: &str, child: &str) -> String {
    if base == "." {
        child.to_string()
    } else {
        format!("{base}/{child}")
    }
}

pub(crate) fn parse_skill_markdown_metadata(
    skill_dir: &str,
    flock_slug: &str,
    markdown: &str,
) -> Result<ScannedSkillMetadata, String> {
    let parsed = parse_frontmatter(markdown);
    let name = parsed
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| extract_markdown_heading(markdown))
        .ok_or_else(|| {
            format!(
                "`{skill_dir}` must define a non-empty `name` in SKILL.md frontmatter or a markdown heading"
            )
        })?;
    let description = extract_summary(&parsed, markdown)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "`{skill_dir}` must define a non-empty `description` in SKILL.md frontmatter or body"
            )
        })?;
    derive_skill_slug(skill_dir, flock_slug, &name, &parsed).ok_or_else(|| {
        format!(
            "`{skill_dir}` did not produce a valid skill slug from its directory name or SKILL.md"
        )
    })?;
    let version = parsed
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);

    Ok(ScannedSkillMetadata {
        name,
        description,
        version,
    })
}

fn derive_skill_slug(
    skill_dir: &str,
    flock_slug: &str,
    name: &str,
    parsed: &Value,
) -> Option<String> {
    let frontmatter_slug = parsed
        .get("slug")
        .and_then(Value::as_str)
        .map(sanitize_skill_slug)
        .filter(|value| !value.is_empty());
    if frontmatter_slug.is_some() {
        return frontmatter_slug;
    }

    let directory_slug = if skill_dir == "." {
        sanitize_skill_slug(flock_slug)
    } else {
        skill_dir
            .rsplit('/')
            .next()
            .map(sanitize_skill_slug)
            .unwrap_or_default()
    };
    if !directory_slug.is_empty() {
        return Some(directory_slug);
    }

    let fallback_slug = sanitize_skill_slug(name);
    if fallback_slug.is_empty() {
        None
    } else {
        Some(fallback_slug)
    }
}

pub(crate) fn sanitize_skill_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;

    for ch in value.trim().chars().flat_map(char::to_lowercase) {
        let keep = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        if keep {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }

    while slug.starts_with('-') {
        slug.remove(0);
    }
    while slug.ends_with('-') {
        slug.pop();
    }

    slug
}

/// Derive a stable, human-readable cache directory path from a git URL.
///
/// `https://github.com/openclaw/skills.git` → `github.com/openclaw/skills`
///
/// The same repo always maps to the same directory regardless of git_ref.
fn repo_cache_dir_name(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.trim_end_matches(".git");
    let url = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // Sanitize each path segment to avoid illegal filesystem characters
    url.split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn skill_markdown_path(relative_dir: &str) -> String {
    if relative_dir == "." {
        "SKILL.md".to_string()
    } else {
        format!("{relative_dir}/SKILL.md")
    }
}

fn is_skill_markdown_path(path: &str) -> bool {
    path == "SKILL.md" || path.ends_with("/SKILL.md")
}

fn parse_changed_skill_markdown_paths(stdout: &str) -> Vec<String> {
    let mut paths = HashSet::new();

    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let columns = line.split('\t').collect::<Vec<_>>();
        let file_columns = match columns.as_slice() {
            [_, path] => vec![*path],
            [_, old_path, new_path] => vec![*old_path, *new_path],
            _ => continue,
        };

        for path in file_columns {
            if is_skill_markdown_path(path) {
                paths.insert(path.replace('\\', "/"));
            }
        }
    }

    let mut paths = paths.into_iter().collect::<Vec<_>>();
    paths.sort();
    paths
}

fn collect_skill_markdown_files(repo_dir: &Path) -> Result<Vec<String>, AppError> {
    let mut candidates = Vec::new();
    collect_skill_candidates(repo_dir, ".", &mut candidates)?;
    let mut files = candidates
        .into_iter()
        .map(|candidate| skill_markdown_path(&candidate.relative_dir))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

pub(crate) async fn changed_skill_markdown_files(
    repo_dir: &Path,
    previous_sha: &str,
    current_sha: &str,
) -> Result<Vec<String>, AppError> {
    if previous_sha == current_sha {
        return Ok(Vec::new());
    }

    let output = tokio::process::Command::new("git")
        .args([
            "diff",
            "--name-status",
            "--find-renames",
            previous_sha,
            current_sha,
            "--",
        ])
        .current_dir(repo_dir)
        .output()
        .await
        .map_err(|error| AppError::Internal(format!("failed to run git diff: {error}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(format!("git diff failed: {stderr}")));
    }

    Ok(parse_changed_skill_markdown_paths(
        &String::from_utf8_lossy(&output.stdout),
    ))
}

/// Return **all** file paths changed between two commits (not just SKILL.md).
/// Paths are relative to the repo root, forward-slash separated and deduplicated.
pub(crate) async fn changed_files_between(
    repo_dir: &Path,
    previous_sha: &str,
    current_sha: &str,
) -> Result<Vec<String>, AppError> {
    if previous_sha == current_sha {
        return Ok(Vec::new());
    }

    let output = tokio::process::Command::new("git")
        .args(["diff", "--name-only", previous_sha, current_sha, "--"])
        .current_dir(repo_dir)
        .output()
        .await
        .map_err(|error| AppError::Internal(format!("failed to run git diff: {error}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(format!("git diff failed: {stderr}")));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut paths: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.replace('\\', "/"))
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(crate) fn cached_repo_dir(base_path: &Path, url: &str) -> PathBuf {
    base_path.join(repo_cache_dir_name(url))
}

pub(crate) async fn refresh_cached_repo(
    base_path: &Path,
    url: &str,
    git_ref: &str,
) -> Result<CachedRepoCheckout, AppError> {
    fs::create_dir_all(base_path).map_err(|error| {
        AppError::Internal(format!(
            "failed to create repo cache directory `{}`: {error}",
            base_path.display()
        ))
    })?;

    let repo_dir = cached_repo_dir(base_path, url);

    let lock = app_state().repo_checkout_lock(&repo_dir);
    let _guard = lock.lock().await;

    let git_dir = repo_dir.join(".git");

    if repo_dir.exists() && !git_dir.is_dir() {
        tracing::warn!(
            "cached repo path `{}` exists but is not a git checkout, removing and re-cloning",
            repo_dir.display()
        );
        fs::remove_dir_all(&repo_dir).map_err(|error| {
            AppError::Internal(format!(
                "failed to remove broken repo cache `{}`: {error}",
                repo_dir.display()
            ))
        })?;
    }

    if git_dir.is_dir() {
        let previous_sha = get_head_sha(&repo_dir).await?;
        let pull_result = pull_git_repo(&repo_dir, git_ref).await?;
        let changed_skill_files =
            changed_skill_markdown_files(&repo_dir, &previous_sha, &pull_result.head_sha).await?;
        return Ok(CachedRepoCheckout {
            path: repo_dir,
            head_sha: pull_result.head_sha,
            previous_sha: Some(previous_sha),
            changed_skill_files,
            reused: true,
            redirected_url: pull_result.redirected_url,
        });
    }

    let clone_result = clone_git_repo(url, git_ref, &repo_dir).await?;
    let changed_skill_files = collect_skill_markdown_files(&repo_dir)?;
    Ok(CachedRepoCheckout {
        path: repo_dir,
        head_sha: clone_result.head_sha,
        previous_sha: None,
        changed_skill_files,
        reused: false,
        redirected_url: clone_result.redirected_url,
    })
}

fn extract_markdown_heading(markdown: &str) -> Option<String> {
    markdown.lines().find_map(|line| {
        let trimmed = line.trim();
        let title = trimmed.trim_start_matches('#').trim();
        if trimmed.starts_with('#') && !title.is_empty() && !is_decorative_line(title) {
            Some(title.to_string())
        } else {
            None
        }
    })
}

/// Lines like `===============` or `***************` are decorative separators,
/// not real headings.
fn is_decorative_line(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| matches!(c, '=' | '-' | '*' | '_' | '~' | '#'))
}

/// Result of a git clone or pull operation.
pub(crate) struct GitOpResult {
    pub head_sha: String,
    /// If git followed an HTTP redirect (repo moved/renamed), the new URL.
    pub redirected_url: Option<String>,
}

/// Parse git stderr for redirect warnings (HTTP 301/302).
/// Git outputs: `warning: redirecting to <url>`
fn parse_git_redirect(stderr: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stderr);
    for line in text.lines() {
        if let Some(url) = line.strip_prefix("warning: redirecting to ") {
            let url = url.trim().trim_end_matches('/');
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    None
}

pub(crate) async fn clone_git_repo(
    url: &str,
    git_ref: &str,
    target_dir: &Path,
) -> Result<GitOpResult, AppError> {
    fs::create_dir_all(target_dir).map_err(|error| {
        AppError::Internal(format!(
            "failed to create clone target directory `{}`: {error}",
            target_dir.display()
        ))
    })?;
    let mut args = vec!["clone", "--depth", "1", "--single-branch"];
    if !git_ref.is_empty() && !git_ref.eq_ignore_ascii_case("HEAD") {
        args.push("--branch");
        args.push(git_ref);
    }
    args.push(url);
    let lossy = target_dir.to_string_lossy();
    args.push(&lossy);
    let output = tokio::process::Command::new("git")
        .args(&args)
        .output()
        .await
        .map_err(|error| AppError::Internal(format!("failed to run git clone: {error}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if cfg!(windows) && stderr.contains("Clone succeeded, but checkout failed") {
            tracing::warn!(
                "git clone checkout failed (invalid Windows filename), configuring sparse-checkout"
            );
            setup_windows_sparse_checkout(target_dir).await;
            let _ = tokio::process::Command::new("git")
                .args(["reset", "--hard", "HEAD"])
                .current_dir(target_dir)
                .output()
                .await;
        } else {
            return Err(AppError::BadRequest(format!("git clone failed: {stderr}")));
        }
    }

    let redirected_url = parse_git_redirect(&output.stderr);
    if let Some(ref new_url) = redirected_url {
        tracing::warn!(
            "git clone followed a redirect: {url} -> {new_url} (repo may have been moved/renamed)"
        );
    }

    let head_sha = get_head_sha(target_dir).await?;
    Ok(GitOpResult {
        head_sha,
        redirected_url,
    })
}

pub(crate) async fn pull_git_repo(repo_dir: &Path, git_ref: &str) -> Result<GitOpResult, AppError> {
    let fetch_args: Vec<&str> = if git_ref.is_empty() || git_ref.eq_ignore_ascii_case("HEAD") {
        vec!["fetch", "--prune", "origin"]
    } else {
        vec!["fetch", "--prune", "origin", git_ref]
    };

    let fetch_output = tokio::process::Command::new("git")
        .args(&fetch_args)
        .current_dir(repo_dir)
        .output()
        .await
        .map_err(|error| AppError::Internal(format!("failed to run git fetch: {error}")))?;

    if !fetch_output.status.success() {
        let stderr = String::from_utf8_lossy(&fetch_output.stderr);
        return Err(AppError::Internal(format!("git fetch failed: {stderr}")));
    }

    let redirected_url = parse_git_redirect(&fetch_output.stderr);
    if let Some(ref new_url) = redirected_url {
        tracing::warn!(
            "git fetch followed a redirect to {new_url} (repo may have been moved/renamed)"
        );
    }

    let reset_target = if git_ref.is_empty() || git_ref.eq_ignore_ascii_case("HEAD") {
        tokio::process::Command::new("git")
            .args(["symbolic-ref", "refs/remotes/origin/HEAD", "--short"])
            .current_dir(repo_dir)
            .output()
            .await
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if branch.is_empty() {
                    None
                } else {
                    Some(branch)
                }
            })
            .unwrap_or_else(|| "origin/main".to_string())
    } else {
        "FETCH_HEAD".to_string()
    };

    setup_windows_sparse_checkout(repo_dir).await;

    let reset_output = tokio::process::Command::new("git")
        .args(["reset", "--hard", &reset_target])
        .current_dir(repo_dir)
        .output()
        .await
        .map_err(|error| AppError::Internal(format!("failed to run git reset: {error}")))?;

    if !reset_output.status.success() {
        let stderr = String::from_utf8_lossy(&reset_output.stderr);
        if cfg!(windows) && stderr.contains("invalid path") {
            tracing::warn!("git reset had invalid-path warnings (non-fatal on Windows): {stderr}");
        } else {
            return Err(AppError::Internal(format!(
                "git reset --hard {reset_target} failed: {stderr}"
            )));
        }
    }

    let clean_output = tokio::process::Command::new("git")
        .args(["clean", "-ffdx"])
        .current_dir(repo_dir)
        .output()
        .await
        .map_err(|error| AppError::Internal(format!("failed to run git clean: {error}")))?;

    if !clean_output.status.success() {
        let stderr = String::from_utf8_lossy(&clean_output.stderr);
        return Err(AppError::Internal(format!("git clean failed: {stderr}")));
    }

    let head_sha = get_head_sha(repo_dir).await?;
    Ok(GitOpResult {
        head_sha,
        redirected_url,
    })
}

/// Resolve a remote ref to its commit SHA without cloning.
pub(crate) async fn resolve_remote_sha(url: &str, git_ref: &str) -> Result<String, AppError> {
    let output = tokio::process::Command::new("git")
        .args(["ls-remote", url, git_ref])
        .output()
        .await
        .map_err(|error| AppError::Internal(format!("failed to run git ls-remote: {error}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::BadRequest(format!(
            "git ls-remote failed: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .next()
        .and_then(|line| line.split('\t').next())
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
        .ok_or_else(|| AppError::BadRequest(format!("ref `{git_ref}` not found in remote repo")))
}

async fn get_head_sha(repo_dir: &Path) -> Result<String, AppError> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_dir)
        .output()
        .await
        .map_err(|error| AppError::Internal(format!("failed to get HEAD sha: {error}")))?;

    if !output.status.success() {
        return Err(AppError::Internal(
            "failed to get HEAD commit SHA".to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn collect_skill_candidates_finds_nested_skill_directories() {
        let root = std::env::temp_dir().join(format!("savhub-git-ops-test-{}", Uuid::now_v7()));
        fs::create_dir_all(root.join("skills").join("python")).expect("create dirs");
        fs::write(
            root.join("skills").join("python").join("SKILL.md"),
            "---\nname: Python\ndescription: Python tools.\n---\n# Python",
        )
        .expect("write skill");

        let mut candidates = Vec::new();
        collect_skill_candidates(&root, ".", &mut candidates).expect("scan");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].relative_dir, "skills/python");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parse_skill_markdown_metadata_reads_frontmatter_without_meta_toml() {
        let markdown = "---\nname: Prompt Crafter\ndescription: Build prompts from templates.\n---\n# Prompt Crafter\n";

        let metadata = parse_skill_markdown_metadata("skills/prompt-crafter", "tools", markdown)
            .expect("parse markdown");

        assert_eq!(metadata.name, "Prompt Crafter");
        assert_eq!(metadata.description, "Build prompts from templates.");
    }

    #[test]
    fn parse_skill_markdown_metadata_falls_back_to_heading_and_body() {
        let markdown = "# Shell Runner\n\nExecute shell tasks safely.\n";

        let metadata =
            parse_skill_markdown_metadata(".", "shell-runner", markdown).expect("parse markdown");

        assert_eq!(metadata.name, "Shell Runner");
        assert_eq!(metadata.description, "Execute shell tasks safely.");
    }

    #[test]
    fn cached_repo_dir_is_stable_per_url() {
        let base = PathBuf::from("repos");
        let a = cached_repo_dir(&base, "https://github.com/acme/skills.git");
        let b = cached_repo_dir(&base, "https://github.com/acme/skills.git");
        // Same repo with different ref should produce the same directory
        let c = cached_repo_dir(&base, "https://github.com/acme/other.git");

        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, PathBuf::from("repos/github.com/acme/skills"));
    }

    #[test]
    fn parse_changed_skill_markdown_paths_handles_edits_and_renames() {
        let stdout = "\
M\tskills/python/SKILL.md\n\
R100\ttools/old/SKILL.md\ttools/new/SKILL.md\n\
D\tREADME.md\n";

        let paths = parse_changed_skill_markdown_paths(stdout);

        assert_eq!(
            paths,
            vec![
                "skills/python/SKILL.md".to_string(),
                "tools/new/SKILL.md".to_string(),
                "tools/old/SKILL.md".to_string(),
            ]
        );
    }
}
