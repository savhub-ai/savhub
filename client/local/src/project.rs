use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::clients::home_dir;
use crate::skills::{
    LockSkill, Lockfile, RepoSkillFolder, RepoSkillOrigin, SkillFolder, copy_skill_folder,
    find_repo_skill_folders, find_skill_folders, read_skill_version_info, repo_git_sha,
    skill_folder_from_path, write_repo_skill_origin,
};
use crate::utils::sanitize_slug;

/// A selector that matched for a project and the flocks/skills it contributed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSelectorMatch {
    #[serde(alias = "selector")]
    pub selector: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flocks: Vec<crate::selectors::SelectorSkillRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<crate::selectors::SelectorSkillRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<crate::selectors::SelectorRepo>,
}

/// Selectors section in savhub.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectSelectorsConfig {
    /// Selectors matched by `savhub apply` (auto-managed).
    #[serde(default)]
    pub matched: Vec<ProjectSelectorMatch>,
    /// User-manually-added selectors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_added: Vec<String>,
    /// User-manually-skipped selectors (never match).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_skipped: Vec<String>,
}

/// A manually added skill, identified by repo URL + path.
///
/// Validation: a valid entry must have both `path` and `slug` non-empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectAddedSkill {
    /// Registry path that uniquely identifies the skill.
    #[serde(default)]
    pub path: String,
    /// Skill slug in the registry (e.g. `claude-api`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slug: String,
    /// Git URL of the repo this skill was fetched from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Local path relative to the project `skills/` directory.
    /// Only set when the local directory name differs from the slug
    /// (e.g. due to conflict resolution or flock-grouped layout).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub fetched_at: i64,
}

/// How skills are organized in the project `skills/` directory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLayout {
    /// Flat: `skills/{slug}/` (default)
    #[default]
    Flat,
    /// Grouped by flock: `skills/{flock_slug}/{skill_slug}/`
    Flock,
}

/// Skills section in savhub.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectSkillsConfig {
    /// Directory layout for fetched skills.
    #[serde(default, skip_serializing_if = "is_default_layout")]
    pub layout: SkillLayout,
    /// User-manually-added skills.
    #[serde(default, alias = "added")]
    pub manual_added: Vec<ProjectAddedSkill>,
    /// Skill slugs that should never be auto-fetched.
    #[serde(default, alias = "skipped")]
    pub manual_skipped: Vec<String>,
}

/// Flocks section in savhub.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectFlocksConfig {
    /// Flocks contributed by matched selectors (auto-managed).
    #[serde(default)]
    pub matched: Vec<crate::selectors::SelectorSkillRef>,
    /// User-manually-added flocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_added: Vec<String>,
    /// User-manually-skipped flocks (never fetch).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_skipped: Vec<String>,
}

fn is_default_layout(layout: &SkillLayout) -> bool {
    *layout == SkillLayout::Flat
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfigFile {
    pub version: u8,
    #[serde(default)]
    pub selectors: ProjectSelectorsConfig,
    #[serde(default)]
    pub flocks: ProjectFlocksConfig,
    #[serde(default)]
    pub skills: ProjectSkillsConfig,
}

impl Default for ProjectConfigFile {
    fn default() -> Self {
        Self {
            version: 1,
            selectors: ProjectSelectorsConfig::default(),
            flocks: ProjectFlocksConfig::default(),
            skills: ProjectSkillsConfig::default(),
        }
    }
}

/// A locked skill entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectLockedSkill {
    /// Git URL of the repo this skill comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Path within the repo (e.g. `skills/salvo-auth`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Skill slug.
    #[serde(default)]
    pub slug: String,
    /// Skill version if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Git revision hash of the fetched commit.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "commit_hash"
    )]
    pub git_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectLockFile {
    pub version: u8,
    pub skills: Vec<ProjectLockedSkill>,
}

impl Default for ProjectLockFile {
    fn default() -> Self {
        Self {
            version: 1,
            skills: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnableProjectRepoSkillResult {
    pub slug: String,
    /// The local folder name under `skills/`. Differs from `slug` when auto-renamed to avoid
    /// conflicts.
    pub local_name: String,
    pub version: Option<String>,
    pub git_sha: Option<String>,
}

// ---------------------------------------------------------------------------
// ProjectConfig I/O
// ---------------------------------------------------------------------------

fn project_config_path(workdir: &Path) -> PathBuf {
    workdir.join("savhub.toml")
}

fn project_lock_path(workdir: &Path) -> PathBuf {
    workdir.join("savhub.lock")
}

/// Return the project-level skills directories for all installed AI clients
/// (e.g. `.claude/skills`, `.agents/skills`).
fn installed_client_skills_dirs(workdir: &Path) -> Vec<PathBuf> {
    crate::clients::detect_clients()
        .iter()
        .filter(|c| c.installed)
        .filter_map(|c| c.kind.project_skills_dir())
        .map(|rel| workdir.join(rel))
        .collect()
}

/// Compute the local directory name for a skill given the layout.
///
/// - `Flat`: `{slug}` (or `{slug}-2`, `{slug}-3` on conflict)
/// - `Flock`: `{flock_slug}/{slug}` (or `{flock_slug}/{slug}-2` on conflict)
///
/// Returns `(local_path, renamed)` where `renamed` is true if a suffix was added.
pub fn compute_skill_local_path(
    skills_dir: &Path,
    slug: &str,
    flock_slug: Option<&str>,
    layout: SkillLayout,
) -> (String, bool) {
    let base_dir = match layout {
        SkillLayout::Flock => {
            let flock = flock_slug.unwrap_or("_default");
            format!("{flock}/{slug}")
        }
        SkillLayout::Flat => slug.to_string(),
    };

    // Try without suffix first
    if !skills_dir.join(&base_dir).exists() {
        return (base_dir, false);
    }

    // Conflict: append -{num}
    for i in 2..100 {
        let candidate = match layout {
            SkillLayout::Flock => {
                let flock = flock_slug.unwrap_or("_default");
                format!("{flock}/{slug}-{i}")
            }
            SkillLayout::Flat => format!("{slug}-{i}"),
        };
        if !skills_dir.join(&candidate).exists() {
            return (candidate, true);
        }
    }

    // Fallback (should never happen)
    (base_dir, false)
}

/// Read the skill layout from the project's savhub.toml.
pub fn read_project_skill_layout(workdir: &Path) -> SkillLayout {
    read_project_config(workdir)
        .map(|c| c.skills.layout)
        .unwrap_or_default()
}

pub fn repo_checkout_root() -> PathBuf {
    home_dir().join(".savhub").join("repos")
}

fn dedup_skill_refs(
    refs: &[crate::selectors::SelectorSkillRef],
) -> Vec<crate::selectors::SelectorSkillRef> {
    let mut seen = std::collections::BTreeSet::new();
    refs.iter()
        .filter(|r| !r.repo.trim().is_empty() && seen.insert((*r).clone()))
        .cloned()
        .collect()
}

fn normalize_selector_matches(matches: &[ProjectSelectorMatch]) -> Vec<ProjectSelectorMatch> {
    let mut normalized = Vec::new();
    for matched in matches {
        let selector = matched.selector.trim().to_string();
        if selector.is_empty() {
            continue;
        }
        let flocks = dedup_skill_refs(&matched.flocks);
        let skills = dedup_skill_refs(&matched.skills);
        let repos = {
            let mut seen = std::collections::BTreeSet::new();
            matched
                .repos
                .iter()
                .filter(|r| !r.git_url.trim().is_empty() && seen.insert(r.git_url.clone()))
                .cloned()
                .collect::<Vec<_>>()
        };
        let duplicate = normalized
            .iter()
            .any(|existing: &ProjectSelectorMatch| existing.selector == selector);
        if !duplicate {
            normalized.push(ProjectSelectorMatch {
                selector,
                flocks,
                skills,
                repos,
            });
        }
    }
    normalized
}

fn normalize_added_skills(skills: &[ProjectAddedSkill]) -> Vec<ProjectAddedSkill> {
    let mut normalized = Vec::new();
    for skill in skills {
        let path = sanitize_slug(&skill.path);
        let slug = skill.slug.trim().to_string();

        // Validation: must have both path and slug.
        if path.is_empty() || slug.is_empty() {
            continue;
        }

        if let Some(existing) = normalized
            .iter_mut()
            .find(|existing: &&mut ProjectAddedSkill| existing.path == path)
        {
            let existing_empty = existing
                .version
                .as_deref()
                .map(|v| v.trim().is_empty())
                .unwrap_or(true);
            let existing_latest = existing.version.as_deref() == Some("latest");
            if existing_empty || existing_latest {
                existing.version = skill.version.clone();
            }
            existing.fetched_at = existing.fetched_at.max(skill.fetched_at);
            continue;
        }
        normalized.push(ProjectAddedSkill {
            path,
            slug,
            repo: skill.repo.clone(),
            local: skill.local.clone(),
            version: skill.version.clone(),
            fetched_at: skill.fetched_at,
        });
    }
    normalized.sort_by(|left, right| left.path.cmp(&right.path));
    normalized
}

fn normalize_project_lock_skills(skills: &[ProjectLockedSkill]) -> Vec<ProjectLockedSkill> {
    let mut normalized = Vec::new();
    for skill in skills {
        let slug = skill.slug.trim().to_string();
        if slug.is_empty()
            || normalized
                .iter()
                .any(|existing: &ProjectLockedSkill| existing.slug == slug)
        {
            continue;
        }
        let trim_opt = |v: &Option<String>| {
            v.as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        normalized.push(ProjectLockedSkill {
            repo: trim_opt(&skill.repo),
            path: trim_opt(&skill.path),
            slug,
            version: trim_opt(&skill.version),
            git_sha: trim_opt(&skill.git_sha),
        });
    }
    normalized.sort_by(|left, right| left.slug.cmp(&right.slug));
    normalized
}

fn lockfile_to_project_added_skills(lockfile: &Lockfile) -> Vec<ProjectAddedSkill> {
    crate::skills::flatten_lockfile(lockfile)
        .into_iter()
        .map(|e| ProjectAddedSkill {
            path: e.path,
            slug: e.slug,
            repo: Some(e.repo_url),
            local: None,
            version: Some(e.version),
            fetched_at: 0,
        })
        .collect()
}

fn project_added_skills_to_lockfile(skills: &[ProjectAddedSkill]) -> Lockfile {
    let mut lockfile = Lockfile::default();
    for skill in normalize_added_skills(skills) {
        let repo_url = skill.repo.clone().unwrap_or_else(|| "unknown".to_string());
        let version = skill.version.unwrap_or_else(|| "latest".to_string());
        lockfile.insert(
            &repo_url,
            "",
            &skill.path,
            LockSkill {
                path: skill.path.clone(),
                slug: skill.slug,
                version,
            },
        );
    }
    lockfile
}

pub fn read_project_config(workdir: &Path) -> Result<ProjectConfigFile> {
    let path = project_config_path(workdir);
    if let Ok(raw) = fs::read_to_string(&path) {
        let mut config: ProjectConfigFile =
            toml::from_str(&raw).with_context(|| format!("invalid {}", path.display()))?;
        config.selectors.matched = normalize_selector_matches(&config.selectors.matched);
        config.skills.manual_added = normalize_added_skills(&config.skills.manual_added);
        return Ok(config);
    }
    Ok(ProjectConfigFile::default())
}

pub fn write_project_config(workdir: &Path, config: &ProjectConfigFile) -> Result<()> {
    write_project_config_inner(workdir, config, false)
}

/// Write project config, always creating the file even if all sections are empty.
pub fn write_project_config_force(workdir: &Path, config: &ProjectConfigFile) -> Result<()> {
    write_project_config_inner(workdir, config, true)
}

fn write_project_config_inner(
    workdir: &Path,
    config: &ProjectConfigFile,
    force: bool,
) -> Result<()> {
    let path = project_config_path(workdir);
    let mut normalized = config.clone();
    normalized.version = 1;
    normalized.selectors.matched = normalize_selector_matches(&normalized.selectors.matched);
    normalized.skills.manual_added = normalize_added_skills(&normalized.skills.manual_added);

    if !force
        && normalized.selectors.matched.is_empty()
        && normalized.selectors.manual_added.is_empty()
        && normalized.selectors.manual_skipped.is_empty()
        && normalized.skills.manual_added.is_empty()
        && normalized.skills.manual_skipped.is_empty()
        && normalized.flocks.matched.is_empty()
        && normalized.flocks.manual_added.is_empty()
        && normalized.flocks.manual_skipped.is_empty()
    {
        if path.exists() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let payload = toml::to_string_pretty(&normalized)?;
    fs::write(path, format!("{payload}\n"))?;
    Ok(())
}

pub fn read_project_lockfile(workdir: &Path) -> Result<ProjectLockFile> {
    let path = project_lock_path(workdir);
    let Ok(raw) = fs::read_to_string(&path) else {
        return Ok(ProjectLockFile::default());
    };
    let mut lockfile: ProjectLockFile =
        toml::from_str(&raw).with_context(|| format!("invalid {}", path.display()))?;
    lockfile.version = 1;
    lockfile.skills = normalize_project_lock_skills(&lockfile.skills);
    Ok(lockfile)
}

pub fn write_project_lockfile(workdir: &Path, lockfile: &ProjectLockFile) -> Result<()> {
    write_project_lockfile_inner(workdir, lockfile, false)
}

/// Write lockfile, always creating the file even if skills list is empty.
pub fn write_project_lockfile_force(workdir: &Path, lockfile: &ProjectLockFile) -> Result<()> {
    write_project_lockfile_inner(workdir, lockfile, true)
}

fn write_project_lockfile_inner(
    workdir: &Path,
    lockfile: &ProjectLockFile,
    force: bool,
) -> Result<()> {
    let path = project_lock_path(workdir);
    let mut normalized = lockfile.clone();
    normalized.version = 1;
    normalized.skills = normalize_project_lock_skills(&normalized.skills);

    if !force && normalized.skills.is_empty() {
        if path.exists() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }

    let payload = toml::to_string_pretty(&normalized)?;
    fs::write(path, format!("{payload}\n"))?;
    Ok(())
}

pub fn read_project_selector_matches(workdir: &Path) -> Result<Vec<ProjectSelectorMatch>> {
    Ok(read_project_config(workdir)?.selectors.matched)
}

pub fn read_project_added_skills(workdir: &Path) -> Result<Lockfile> {
    Ok(project_added_skills_to_lockfile(
        &read_project_config(workdir)?.skills.manual_added,
    ))
}

fn upsert_project_added_skill(workdir: &Path, skill: ProjectAddedSkill) -> Result<()> {
    let mut config = read_project_config(workdir)?;
    if let Some(existing) = config
        .skills
        .manual_added
        .iter_mut()
        .find(|existing| existing.path == skill.path)
    {
        existing.version = skill.version;
        existing.fetched_at = skill.fetched_at;
    } else {
        config.skills.manual_added.push(skill);
    }
    write_project_config(workdir, &config)?;
    sync_project_lock(workdir)
}

fn remove_project_added_skill(workdir: &Path, slug: &str) -> Result<()> {
    let mut config = read_project_config(workdir)?;
    config
        .skills
        .manual_added
        .retain(|skill| skill.path != slug);
    write_project_config(workdir, &config)?;
    sync_project_lock(workdir)
}

pub fn write_project_added_skills(workdir: &Path, lockfile: &Lockfile) -> Result<()> {
    let mut config = read_project_config(workdir)?;
    config.skills.manual_added = lockfile_to_project_added_skills(lockfile);
    write_project_config(workdir, &config)?;
    sync_project_lock(workdir)
}

fn find_repo_skill(repo_name: &str, slug: &str) -> Result<RepoSkillFolder> {
    let repo_name = repo_name.trim();
    let slug = sanitize_slug(slug);
    if repo_name.is_empty() || slug.is_empty() {
        bail!("invalid repo skill selection");
    }

    list_repo_skills()?
        .into_iter()
        .find(|candidate| {
            candidate.repo_name.eq_ignore_ascii_case(repo_name) && candidate.skill.slug == slug
        })
        .with_context(|| format!("skill '{slug}' not found in repo '{repo_name}'"))
}

pub fn enable_repo_skill_in_project(
    workdir: &Path,
    repo_name: &str,
    slug: &str,
) -> Result<EnableProjectRepoSkillResult> {
    let repo_skill = find_repo_skill(repo_name, slug)?;
    let slug = repo_skill.skill.slug.clone();

    // Copy to each installed AI client's project skills directory
    for dir in installed_client_skills_dirs(workdir) {
        let target = dir.join(&slug);
        fs::create_dir_all(&dir)?;
        copy_skill_folder(&repo_skill.skill.folder, &target)?;

        let mut version_info =
            read_skill_version_info(&repo_skill.skill.folder).unwrap_or_default();
        if version_info.git_sha.is_none() {
            version_info.git_sha = repo_git_sha(&repo_skill.repo_root);
        }
        let fetched_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let _ = write_repo_skill_origin(
            &target,
            &RepoSkillOrigin {
                version: 1,
                repo: repo_skill.repo_name.clone(),
                repo_sign: repo_skill.repo_root.display().to_string(),
                repo_commit: version_info.git_sha.clone(),
                slug: slug.clone(),
                skill_version: version_info.version.clone(),
                fetched_at,
            },
        );
    }

    let mut version_info = read_skill_version_info(&repo_skill.skill.folder).unwrap_or_default();
    if version_info.git_sha.is_none() {
        version_info.git_sha = repo_git_sha(&repo_skill.repo_root);
    }
    let fetched_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    upsert_project_added_skill(
        workdir,
        ProjectAddedSkill {
            path: slug.clone(),
            slug: slug.clone(),
            repo: None,
            local: None,
            version: version_info.version.clone(),
            fetched_at,
        },
    )?;

    Ok(EnableProjectRepoSkillResult {
        slug: slug.clone(),
        local_name: slug,
        version: version_info.version,
        git_sha: version_info.git_sha,
    })
}

pub fn enable_fetched_skill_in_project(
    workdir: &Path,
    repo_url: &str,
    skill_path: &str,
    slug: &str,
) -> Result<EnableProjectRepoSkillResult> {
    let slug = sanitize_slug(slug);
    if slug.is_empty() {
        bail!("invalid skill slug");
    }

    let config_dir = crate::config::get_config_dir()?;
    let stripped = repo_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .strip_prefix("https://")
        .or_else(|| {
            repo_url
                .trim()
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .strip_prefix("http://")
        })
        .unwrap_or(repo_url.trim());
    let skill_folder = config_dir.join("repos").join(stripped).join(skill_path);

    if !skill_folder.is_dir() {
        bail!("fetched skill not found at {}", skill_folder.display());
    }

    // Copy to each installed AI client's project skills directory
    let version_info = read_skill_version_info(&skill_folder).unwrap_or_default();
    let fetched_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    for dir in installed_client_skills_dirs(workdir) {
        let target = dir.join(&slug);
        fs::create_dir_all(&dir)?;
        copy_skill_folder(&skill_folder, &target)?;
        let _ = write_repo_skill_origin(
            &target,
            &RepoSkillOrigin {
                version: 1,
                repo: repo_url.to_string(),
                repo_sign: repo_url.to_string(),
                repo_commit: version_info.git_sha.clone(),
                slug: slug.clone(),
                skill_version: version_info.version.clone(),
                fetched_at,
            },
        );
    }

    upsert_project_added_skill(
        workdir,
        ProjectAddedSkill {
            path: slug.clone(),
            slug: slug.clone(),
            repo: Some(repo_url.to_string()),
            local: None,
            version: version_info.version.clone(),
            fetched_at,
        },
    )?;

    Ok(EnableProjectRepoSkillResult {
        slug: slug.clone(),
        local_name: slug,
        version: version_info.version,
        git_sha: version_info.git_sha,
    })
}

/// Manually attach a flock from the central fetched cache to this project.
///
/// Adds `repo_url:flock_path` to `flocks.manual_added` (so it survives `apply`)
/// and copies every skill in the flock from `~/.savhub/repos/...` into each
/// detected AI client's project skill directory. The lockfile is updated for
/// every skill that materialised on disk; missing repo cache entries are
/// reported back to the caller as a warning list rather than a hard error.
pub fn enable_fetched_flock_in_project(
    workdir: &Path,
    repo_url: &str,
    flock_slug: &str,
) -> Result<Vec<String>> {
    let repo_url = repo_url.trim();
    let flock_slug = flock_slug.trim();
    if repo_url.is_empty() || flock_slug.is_empty() {
        bail!("invalid flock identifier");
    }

    let central_lock = {
        let central = crate::config::get_config_dir()?;
        crate::skills::read_lockfile(&central)?
    };
    let flock_skills = central_lock
        .repos
        .iter()
        .find(|repo| repo.git_url == repo_url)
        .and_then(|repo| {
            repo.flocks
                .iter()
                .find(|flock| flock.path == flock_slug)
        });

    let token = format!("{repo_url}:{flock_slug}");
    let mut config = read_project_config(workdir)?;
    if !config.flocks.manual_added.iter().any(|f| f == &token) {
        config.flocks.manual_added.push(token.clone());
    }
    config
        .flocks
        .manual_skipped
        .retain(|entry| entry != &token && entry != flock_slug);
    write_project_config_force(workdir, &config)?;

    let mut warnings = Vec::new();
    let Some(flock_entry) = flock_skills else {
        warnings.push(format!(
            "flock `{flock_slug}` is not present in the local cache; run `savhub fetch` for one of its skills first"
        ));
        return Ok(warnings);
    };

    let stripped = repo_url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .strip_prefix("https://")
        .or_else(|| {
            repo_url
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .strip_prefix("http://")
        })
        .unwrap_or(repo_url);
    let central = crate::config::get_config_dir()?;
    let repo_root = central.join("repos").join(stripped);

    let fetched_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    for entry in &flock_entry.skills {
        let source = repo_root.join(&entry.path);
        if !source.is_dir() {
            warnings.push(format!(
                "skill `{}` cache missing at {}",
                entry.slug,
                source.display()
            ));
            continue;
        }
        let version_info = read_skill_version_info(&source).unwrap_or_default();
        for dir in installed_client_skills_dirs(workdir) {
            let target = dir.join(&entry.slug);
            fs::create_dir_all(&dir)?;
            copy_skill_folder(&source, &target)?;
            let _ = write_repo_skill_origin(
                &target,
                &RepoSkillOrigin {
                    version: 1,
                    repo: repo_url.to_string(),
                    repo_sign: repo_url.to_string(),
                    repo_commit: version_info.git_sha.clone(),
                    slug: entry.slug.clone(),
                    skill_version: version_info.version.clone(),
                    fetched_at,
                },
            );
        }

        let mut lock = read_project_lockfile(workdir).unwrap_or_default();
        if !lock.skills.iter().any(|s| s.slug == entry.slug) {
            lock.skills.push(ProjectLockedSkill {
                repo: Some(repo_url.to_string()),
                path: Some(entry.path.clone()),
                slug: entry.slug.clone(),
                version: version_info.version.clone(),
                git_sha: version_info.git_sha.clone(),
            });
            write_project_lockfile_force(workdir, &lock)?;
        }
    }

    Ok(warnings)
}

pub fn disable_project_skill(workdir: &Path, slug: &str) -> Result<bool> {
    let slug = sanitize_slug(slug);
    if slug.is_empty() {
        bail!("invalid skill slug");
    }

    let mut removed_any = false;
    for dir in installed_client_skills_dirs(workdir) {
        let target = dir.join(&slug);
        if target.exists() {
            fs::remove_dir_all(&target)
                .with_context(|| format!("failed to remove {}", target.display()))?;
            removed_any = true;
        }
    }

    let config = read_project_config(workdir)?;
    let had_manual_entry = config
        .skills
        .manual_added
        .iter()
        .any(|skill| skill.path == slug);
    if had_manual_entry {
        remove_project_added_skill(workdir, &slug)?;
        removed_any = true;
    } else if removed_any {
        sync_project_lock(workdir)?;
    }

    Ok(removed_any)
}

// ---------------------------------------------------------------------------
// Skill resolution
// ---------------------------------------------------------------------------

/// Information about a resolved skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    pub slug: String,
    pub display_name: String,
    pub folder: PathBuf,
}

/// Provenance for an enabled skill in a project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResolvedSkillSources {
    pub selectors: Vec<String>,
    pub flocks: Vec<String>,
    pub manual: bool,
}

/// A resolved skill with source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProjectSkill {
    pub slug: String,
    pub display_name: String,
    pub folder: PathBuf,
    pub sources: ResolvedSkillSources,
}

fn add_unique(items: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !items.iter().any(|item| item == value) {
        items.push(value.to_string());
    }
}

pub fn list_repo_skills() -> Result<Vec<RepoSkillFolder>> {
    find_repo_skill_folders(&repo_checkout_root())
}

fn collect_skill_folders(workdir: &Path) -> Vec<SkillFolder> {
    let mut all_folders: Vec<SkillFolder> = Vec::new();

    // Search installed AI client project-level skill directories
    for dir in installed_client_skills_dirs(workdir) {
        if dir.is_dir() {
            for folder in find_skill_folders(&dir).unwrap_or_default() {
                if !all_folders
                    .iter()
                    .any(|existing| existing.slug == folder.slug)
                {
                    all_folders.push(folder);
                }
            }
        }
    }

    // Repo-fetched skills (from fetched.json)
    let config_dir = crate::config::get_config_dir().unwrap_or_default();
    let lock = crate::skills::read_lockfile(&config_dir).unwrap_or_default();
    for flat in crate::skills::flatten_lockfile(&lock) {
        if let Some(path) = crate::registry::repo_skill_local_path(&flat.repo_url, &flat.path)
            && path.is_dir()
            && let Some(skill) = skill_folder_from_path(&path)
            && !all_folders.iter().any(|e| e.slug == skill.slug)
        {
            all_folders.push(skill);
        }
    }

    all_folders
}

fn build_project_lockfile(
    resolved: &[ResolvedProjectSkill],
    added_skills: &[ProjectAddedSkill],
) -> ProjectLockFile {
    let skills = resolved
        .iter()
        .map(|skill| {
            let version_info = read_skill_version_info(&skill.folder).unwrap_or_default();
            let added = added_skills.iter().find(|a| a.slug == skill.slug);
            ProjectLockedSkill {
                repo: added.and_then(|a| a.repo.clone()),
                path: added.map(|a| a.path.clone()),
                slug: skill.slug.clone(),
                version: version_info.version,
                git_sha: version_info.git_sha,
            }
        })
        .collect::<Vec<_>>();

    ProjectLockFile { version: 1, skills }
}

fn resolve_project_skills_internal(workdir: &Path) -> Result<Vec<ResolvedProjectSkill>> {
    let config = read_project_config(workdir)?;
    let mut sources = BTreeMap::<String, ResolvedSkillSources>::new();

    // Expand skills from matched selectors
    for matched in &config.selectors.matched {
        for skill_ref in &matched.skills {
            let entry = sources.entry(skill_ref.path.clone()).or_default();
            add_unique(&mut entry.selectors, &matched.selector);
        }
        // Expand flocks from each matched selector
        for flock_ref in &matched.flocks {
            if let Ok(skill_slugs) =
                crate::registry::list_flock_skills(&flock_ref.repo, &flock_ref.path)
            {
                for skill_slug in skill_slugs {
                    let entry = sources.entry(skill_slug).or_default();
                    add_unique(&mut entry.flocks, &flock_ref.path);
                    add_unique(&mut entry.selectors, &matched.selector);
                }
            }
        }
        // Expand repos: look up all flocks in each repo, then expand those flocks
        for repo in &matched.repos {
            if let Ok(repo_flocks) = crate::registry::list_repo_flock_refs(&repo.git_url) {
                for flock_ref in &repo_flocks {
                    if let Ok(skill_slugs) =
                        crate::registry::list_flock_skills(&flock_ref.repo, &flock_ref.path)
                    {
                        for skill_slug in skill_slugs {
                            let entry = sources.entry(skill_slug).or_default();
                            add_unique(&mut entry.flocks, &flock_ref.path);
                            add_unique(&mut entry.selectors, &matched.selector);
                        }
                    }
                }
            }
        }
    }

    for skill in &config.skills.manual_added {
        let entry = sources.entry(skill.path.clone()).or_default();
        entry.manual = true;
    }

    // Expand flocks: matched + manual_added, filter out manual_skipped
    let mut all_flock_slugs: Vec<String> = config
        .flocks
        .matched
        .iter()
        .map(|r| r.to_string())
        .collect();
    for slug in &config.flocks.manual_added {
        if !all_flock_slugs.contains(slug) {
            all_flock_slugs.push(slug.clone());
        }
    }
    all_flock_slugs.retain(|s| !config.flocks.manual_skipped.contains(s));
    for flock_slug in &all_flock_slugs {
        let flock_ref = crate::selectors::SelectorSkillRef::parse(flock_slug);
        if let Ok(skill_slugs) =
            crate::registry::list_flock_skills(&flock_ref.repo, &flock_ref.path)
        {
            for skill_slug in skill_slugs {
                let entry = sources.entry(skill_slug).or_default();
                add_unique(&mut entry.flocks, flock_slug);
            }
        }
    }

    let all_folders = collect_skill_folders(workdir);
    let mut resolved = Vec::new();
    for (slug, source) in sources {
        if let Some(folder) = all_folders.iter().find(|candidate| candidate.slug == slug) {
            resolved.push(ResolvedProjectSkill {
                slug: folder.slug.clone(),
                display_name: folder.display_name.clone(),
                folder: folder.folder.clone(),
                sources: source,
            });
        }
    }

    resolved.sort_by(|left, right| left.slug.cmp(&right.slug));
    Ok(resolved)
}

pub fn sync_project_lock(workdir: &Path) -> Result<()> {
    let config = read_project_config(workdir)?;
    let resolved = resolve_project_skills_internal(workdir)?;
    write_project_lockfile(
        workdir,
        &build_project_lockfile(&resolved, &config.skills.manual_added),
    )
}

pub fn resolve_project_skills_with_sources(workdir: &Path) -> Result<Vec<ResolvedProjectSkill>> {
    let config = read_project_config(workdir)?;
    let resolved = resolve_project_skills_internal(workdir)?;
    let _ = write_project_lockfile(
        workdir,
        &build_project_lockfile(&resolved, &config.skills.manual_added),
    );
    Ok(resolved)
}

/// Resolve the list of skills for a project directory.
///
/// Priority:
/// 1. Selector-matched skills and flocks
/// 2. Project-local manual skills from `savhub.toml`
///
/// Skill folders are looked up in the global skills directory.
pub fn resolve_skills_for_project(workdir: &Path) -> Result<Vec<ResolvedSkill>> {
    Ok(resolve_project_skills_with_sources(workdir)?
        .into_iter()
        .map(|skill| ResolvedSkill {
            slug: skill.slug,
            display_name: skill.display_name,
            folder: skill.folder,
        })
        .collect())
}
