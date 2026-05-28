//! Dormant admin commands extracted from `main.rs`.
//!
//! These commands implement publish / moderate / sync / ban / role flows
//! against existing backend endpoints, but they are *not* wired into the
//! `Command` enum and are unreachable from the public CLI surface today.
//!
//! They live in this separate module instead of being deleted because:
//!   - the backend endpoints are stable and these are the canonical clients;
//!   - re-implementing them later would be more expensive than keeping them compiling.
//!
//! To re-enable any of them: add a variant to `Command` in `main.rs`, dispatch
//! to the corresponding `cmd_*` function below, and remove its
//! `#[allow(dead_code)]` attribute.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use dialoguer::Confirm;
use savhub_local::api::ApiClient;
use savhub_local::skills::{
    SkillFolder, compute_fingerprint, ensure_skill_marker, find_skill_folders,
    list_publishable_files, load_local_skill_metadata,
};
use savhub_local::utils::sanitize_slug;
use savhub_shared::{
    BanUserRequest, BanUserResponse, DeleteResponse, IndexRequest, MAX_BUNDLE_BYTES,
    ModerationStatus, ModerationUpdateRequest, PublishBundleFile, PublishResponse, ResolveResponse,
    RoleUpdateResponse, SetUserRoleRequest, SkillDetailResponse, UserListResponse, UserRole,
    is_slug, normalize_bundle_files, normalize_tags, total_bundle_bytes,
};
use semver::Version;

use crate::{
    BanUserArgs, DeleteArgs, GlobalOpts, PublishArgs, SetRoleArgs, SyncArgs, authed_client,
    ensure_confirmed, normalize_slug,
};

#[derive(Debug, Clone)]
pub(crate) struct SyncCandidate {
    skill: SkillFolder,
    local_version: String,
    latest_version: Option<String>,
    matched_version: Option<String>,
    file_count: usize,
    status: SyncStatus,
    issue: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncStatus {
    New,
    Update,
    Synced,
    Blocked,
}

pub(crate) async fn cmd_publish(opts: &GlobalOpts, args: PublishArgs) -> Result<()> {
    publish_folder(
        opts,
        &resolve_folder(&opts.workdir, &args.path)?,
        args.slug.as_deref(),
        args.display_name.as_deref(),
        args.version.as_deref(),
        args.changelog
            .as_deref()
            .unwrap_or("Published via savhub CLI."),
        &args.tags,
    )
    .await
}

pub(crate) async fn cmd_hide(opts: &GlobalOpts, args: DeleteArgs) -> Result<()> {
    moderate_skill(opts, &args, ModerationStatus::Hidden, "Hidden").await
}

pub(crate) async fn cmd_undelete(opts: &GlobalOpts, args: DeleteArgs) -> Result<()> {
    let slug = normalize_slug(&args.slug)?;
    if !args.yes {
        ensure_confirmed(
            opts.input_allowed,
            &format!("Restore {slug}?"),
            "pass --yes when input is disabled",
        )?;
    }
    let client = authed_client(opts)?;
    client
        .post_empty::<DeleteResponse>(&format!("/skills/{slug}/restore"))
        .await?;
    println!("Restored {slug}");
    Ok(())
}

pub(crate) async fn cmd_unhide(opts: &GlobalOpts, args: DeleteArgs) -> Result<()> {
    moderate_skill(opts, &args, ModerationStatus::Active, "Unhidden").await
}

pub(crate) async fn cmd_sync(opts: &GlobalOpts, args: SyncArgs) -> Result<()> {
    let bump = normalize_bump(&args.bump)?;
    let _concurrency = args.concurrency.clamp(1, 32);
    let client = authed_client(opts)?;
    let roots = build_scan_roots(opts, &args.roots);
    let mut by_slug = BTreeMap::<String, SkillFolder>::new();
    for root in roots {
        for skill in find_skill_folders(&root)? {
            by_slug.entry(skill.slug.clone()).or_insert(skill);
        }
    }
    if by_slug.is_empty() {
        println!("No local skills found.");
        return Ok(());
    }

    let mut candidates = Vec::new();
    for skill in by_slug.into_values() {
        let files = list_publishable_files(&skill.folder)?;
        ensure_skill_marker(&files)?;
        let metadata = match load_local_skill_metadata(&files) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => {
                candidates.push(SyncCandidate {
                    skill,
                    local_version: String::new(),
                    latest_version: None,
                    matched_version: None,
                    file_count: files.len(),
                    status: SyncStatus::Blocked,
                    issue: Some("_meta.toml is required for sync".to_string()),
                });
                continue;
            }
            Err(error) => {
                candidates.push(SyncCandidate {
                    skill,
                    local_version: String::new(),
                    latest_version: None,
                    matched_version: None,
                    file_count: files.len(),
                    status: SyncStatus::Blocked,
                    issue: Some(error.to_string()),
                });
                continue;
            }
        };
        let fingerprint = compute_fingerprint(&files);
        let resolved = match resolve_skill_version(&client, &skill.slug, &fingerprint).await {
            Ok(resolved) => resolved,
            Err(error) if error.to_string().contains("404") => ResolveResponse {
                slug: skill.slug.clone(),
                matched: None,
                latest_version: None,
            },
            Err(error) => return Err(error),
        };
        let latest_version = resolved.latest_version.map(|entry| entry.version);
        let matched_version = resolved.matched.map(|entry| entry.version);
        let local_version = metadata.package.version.clone();
        let (status, issue) = if latest_version.is_none() {
            (SyncStatus::New, None)
        } else if matched_version.is_some() {
            (SyncStatus::Synced, None)
        } else if let Some(latest) = latest_version.as_deref() {
            let expected = bump_version(latest, bump)?;
            if local_version == expected {
                (SyncStatus::Update, None)
            } else if local_version == latest {
                (
                    SyncStatus::Blocked,
                    Some(format!(
                        "local files changed but _meta.toml version is still {latest}; expected {expected}"
                    )),
                )
            } else {
                let local = Version::parse(&local_version)
                    .with_context(|| format!("invalid local version: {local_version}"))?;
                let remote = Version::parse(latest)
                    .with_context(|| format!("invalid remote version: {latest}"))?;
                if local <= remote {
                    (
                        SyncStatus::Blocked,
                        Some(format!(
                            "local _meta.toml version {local_version} must be newer than remote {latest}"
                        )),
                    )
                } else {
                    (
                        SyncStatus::Blocked,
                        Some(format!(
                            "local _meta.toml version {local_version} does not match expected {expected} for --bump {bump}"
                        )),
                    )
                }
            }
        } else {
            (
                SyncStatus::Blocked,
                Some("failed to resolve remote version state".to_string()),
            )
        };
        candidates.push(SyncCandidate {
            skill,
            local_version,
            latest_version,
            matched_version,
            file_count: files.len(),
            status,
            issue,
        });
    }

    let blocked = candidates
        .iter()
        .filter(|candidate| candidate.status == SyncStatus::Blocked)
        .cloned()
        .collect::<Vec<_>>();
    if !blocked.is_empty() {
        println!("Blocked:");
        for candidate in &blocked {
            println!(
                "  {}  {}",
                candidate.skill.slug,
                candidate
                    .issue
                    .as_deref()
                    .unwrap_or("invalid local metadata")
            );
        }
    }

    let actionable = candidates
        .iter()
        .filter(|candidate| matches!(candidate.status, SyncStatus::New | SyncStatus::Update))
        .cloned()
        .collect::<Vec<_>>();
    if actionable.is_empty() {
        println!(
            "{}",
            if blocked.is_empty() {
                "Nothing to sync."
            } else {
                "Nothing eligible to sync."
            }
        );
        return Ok(());
    }

    println!("To sync:");
    for candidate in &actionable {
        println!(
            "  {}  {}  (v{} · {} files)",
            candidate.skill.slug,
            sync_status_label(candidate, bump),
            candidate.local_version,
            candidate.file_count
        );
    }

    if args.dry_run {
        println!("Dry run: would upload {} skill(s).", actionable.len());
        return Ok(());
    }

    let selected = if args.all || !opts.input_allowed {
        actionable
    } else {
        let mut selected = Vec::new();
        for candidate in actionable {
            let confirmed = Confirm::new()
                .with_prompt(format!(
                    "Upload {} ({})?",
                    candidate.skill.slug,
                    sync_status_label(&candidate, bump)
                ))
                .default(true)
                .interact()
                .map_err(|error| anyhow!("failed to read confirmation: {error}"))?;
            if confirmed {
                selected.push(candidate);
            }
        }
        selected
    };

    if selected.is_empty() {
        println!("Nothing selected.");
        return Ok(());
    }

    let mut uploaded = 0usize;
    for candidate in selected {
        let changelog = args.changelog.clone().unwrap_or_else(|| {
            if candidate.status == SyncStatus::New {
                "Initial sync import.".to_string()
            } else {
                "Sync update.".to_string()
            }
        });
        publish_folder(
            opts,
            &candidate.skill.folder,
            Some(&candidate.skill.slug),
            Some(&candidate.skill.display_name),
            Some(&candidate.local_version),
            &changelog,
            &args.tags,
        )
        .await?;
        uploaded += 1;
    }

    println!("Uploaded {uploaded} skill(s).");
    Ok(())
}

pub(crate) async fn cmd_ban_user(opts: &GlobalOpts, args: BanUserArgs) -> Result<()> {
    let client = authed_client(opts)?;
    let user_id = resolve_user_id(&client, &args.handle_or_id, args.id, args.fuzzy).await?;
    if !args.yes {
        ensure_confirmed(
            opts.input_allowed,
            &format!("Ban user {}?", args.handle_or_id),
            "pass --yes when input is disabled",
        )?;
    }
    let response = client
        .post_json::<_, BanUserResponse>(
            &format!("/management/users/{user_id}/ban"),
            &BanUserRequest {
                reason: args.reason,
            },
        )
        .await?;
    println!(
        "Banned @{} (revoked {}, deleted {} skills, {} souls)",
        response.user.handle,
        response.revoked_tokens,
        response.deleted_skills,
        response.deleted_skills
    );
    Ok(())
}

pub(crate) async fn cmd_set_role(opts: &GlobalOpts, args: SetRoleArgs) -> Result<()> {
    let role = parse_role_arg(&args.role)?;
    let client = authed_client(opts)?;
    let user_id = resolve_user_id(&client, &args.handle_or_id, args.id, args.fuzzy).await?;
    if !args.yes {
        ensure_confirmed(
            opts.input_allowed,
            &format!("Set role for {} to {:?}?", args.handle_or_id, role),
            "pass --yes when input is disabled",
        )?;
    }
    let response = client
        .post_json::<_, RoleUpdateResponse>(
            &format!("/management/users/{user_id}/role"),
            &SetUserRoleRequest { role },
        )
        .await?;
    println!(
        "Updated @{} -> {:?}",
        response.user.handle, response.user.role
    );
    Ok(())
}

async fn moderate_skill(
    opts: &GlobalOpts,
    args: &DeleteArgs,
    status: ModerationStatus,
    verb: &str,
) -> Result<()> {
    let slug = normalize_slug(&args.slug)?;
    if !args.yes {
        ensure_confirmed(
            opts.input_allowed,
            &format!("{verb} {slug}?"),
            "pass --yes when input is disabled",
        )?;
    }
    let client = authed_client(opts)?;
    client
        .post_json::<_, SkillDetailResponse>(
            &format!("/skills/{slug}/moderation"),
            &ModerationUpdateRequest {
                status,
                highlighted: None,
                official: None,
                deprecated: None,
                suspicious: None,
                notes: None,
            },
        )
        .await?;
    println!("{verb} {slug}");
    Ok(())
}

async fn publish_folder(
    opts: &GlobalOpts,
    folder: &Path,
    slug_arg: Option<&str>,
    display_name_arg: Option<&str>,
    version_arg: Option<&str>,
    changelog: &str,
    tags: &str,
) -> Result<()> {
    let client = authed_client(opts)?;
    let files = list_publishable_files(folder)?;
    if files.is_empty() {
        bail!("no publishable text files found in {}", folder.display());
    }
    ensure_skill_marker(&files)?;
    let metadata = load_local_skill_metadata(&files)?
        .ok_or_else(|| anyhow!("_meta.toml is required for publishing {}", folder.display()))?;

    let slug = metadata.package.slug.clone();
    if !is_slug(&slug) {
        bail!(
            "invalid package.slug in _meta.toml: {}",
            metadata.package.slug
        );
    }
    if let Some(slug_arg) = slug_arg {
        let requested = sanitize_slug(slug_arg);
        if !requested.is_empty() && requested != slug {
            bail!("--slug does not match _meta.toml package.slug ({slug})");
        }
    }

    let display_name = metadata.package.name.clone();
    if let Some(display_name_arg) = display_name_arg {
        let requested = display_name_arg.trim();
        if !requested.is_empty() && requested != display_name {
            bail!("--name does not match _meta.toml package.name ({display_name})");
        }
    }

    let version = metadata.package.version.clone();
    Version::parse(&version).with_context(|| format!("invalid semver: {version}"))?;
    if let Some(version_arg) = version_arg {
        let requested = version_arg.trim();
        if !requested.is_empty() && requested != version {
            bail!("--version does not match _meta.toml package.version ({version})");
        }
    }

    let tags = normalize_tags(
        &tags
            .split(',')
            .map(|tag| tag.trim().to_string())
            .collect::<Vec<_>>(),
    );
    let files = normalize_bundle_files(
        &files
            .into_iter()
            .map(|file| PublishBundleFile {
                path: file.path,
                content: file.content,
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| anyhow!(error))?;
    if total_bundle_bytes(&files) > MAX_BUNDLE_BYTES {
        bail!(
            "bundle exceeds the {}MB upload limit",
            MAX_BUNDLE_BYTES / 1024 / 1024
        );
    }

    let publish_files: Vec<PublishBundleFile> = files
        .into_iter()
        .map(|f| PublishBundleFile {
            path: f.path,
            content: f.content,
        })
        .collect();
    let request = IndexRequest {
        slug: slug.clone(),
        display_name,
        version: version.clone(),
        changelog: if changelog.trim().is_empty() {
            "Published via savhub CLI.".to_string()
        } else {
            changelog.trim().to_string()
        },
        tags,
        files: publish_files,
        summary: Some(metadata.package.description.clone()),
    };
    let response = client
        .post_json::<_, PublishResponse>("/skills", &request)
        .await?;
    println!("Published {}@{} ({})", slug, version, response.version_id);
    Ok(())
}

async fn resolve_skill_version(
    client: &ApiClient,
    slug: &str,
    fingerprint: &str,
) -> Result<ResolveResponse> {
    let mut url = client.v1_url("/resolve")?;
    url.query_pairs_mut()
        .append_pair("slug", slug)
        .append_pair("hash", fingerprint);
    client.get_json_url(url).await
}

fn resolve_folder(workdir: &Path, path: &Path) -> Result<PathBuf> {
    let folder = workdir.join(path);
    let metadata = std::fs::metadata(&folder)
        .with_context(|| format!("path does not exist: {}", folder.display()))?;
    if !metadata.is_dir() {
        bail!("path must be a directory: {}", folder.display());
    }
    Ok(folder)
}

fn build_scan_roots(opts: &GlobalOpts, extra_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut roots = Vec::new();
    for root in std::iter::once(opts.workdir.clone())
        .chain(std::iter::once(opts.dir.clone()))
        .chain(extra_roots.iter().map(|path| opts.workdir.join(path)))
    {
        let normalized = root.canonicalize().unwrap_or(root);
        if seen.insert(normalized.clone()) {
            roots.push(normalized);
        }
    }
    roots
}

fn normalize_bump(value: &str) -> Result<&'static str> {
    match value.trim().to_lowercase().as_str() {
        "patch" => Ok("patch"),
        "minor" => Ok("minor"),
        "major" => Ok("major"),
        _ => bail!("--bump must be patch, minor, or major"),
    }
}

fn bump_version(version: &str, bump: &str) -> Result<String> {
    let parsed = Version::parse(version).with_context(|| format!("invalid semver: {version}"))?;
    let mut next = parsed.clone();
    match bump {
        "major" => {
            next.major += 1;
            next.minor = 0;
            next.patch = 0;
        }
        "minor" => {
            next.minor += 1;
            next.patch = 0;
        }
        _ => {
            next.patch += 1;
        }
    }
    next.pre = semver::Prerelease::EMPTY;
    next.build = semver::BuildMetadata::EMPTY;
    Ok(next.to_string())
}

fn sync_status_label(candidate: &SyncCandidate, bump: &str) -> String {
    match candidate.status {
        SyncStatus::New => "NEW".to_string(),
        SyncStatus::Update => candidate
            .latest_version
            .as_deref()
            .map(|version| format!("UPDATE {version} -> {}", candidate.local_version))
            .unwrap_or_else(|| format!("UPDATE -> {}", candidate.local_version)),
        SyncStatus::Synced => candidate
            .matched_version
            .as_deref()
            .map(|version| format!("SYNCED {version}"))
            .unwrap_or_else(|| "SYNCED".to_string()),
        SyncStatus::Blocked => candidate
            .issue
            .clone()
            .unwrap_or_else(|| format!("BLOCKED ({bump})")),
    }
}

async fn resolve_user_id(
    client: &ApiClient,
    value: &str,
    treat_as_id: bool,
    fuzzy: bool,
) -> Result<String> {
    if treat_as_id {
        return Ok(value.trim().to_string());
    }
    let query = value.trim().trim_start_matches('@');
    let mut url = client.v1_url("/users")?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("limit", "20");
    let result = client.get_json_url::<UserListResponse>(url).await?;
    let exact = result
        .items
        .iter()
        .find(|item| item.user.handle.eq_ignore_ascii_case(query))
        .map(|item| item.user.id.to_string());
    if let Some(exact) = exact {
        return Ok(exact);
    }
    if fuzzy && result.items.len() == 1 {
        return Ok(result.items[0].user.id.to_string());
    }
    bail!("could not resolve user `{value}`")
}

fn parse_role_arg(value: &str) -> Result<UserRole> {
    match value.trim().to_lowercase().as_str() {
        "admin" => Ok(UserRole::Admin),
        "moderator" => Ok(UserRole::Moderator),
        "user" => Ok(UserRole::User),
        _ => bail!("role must be one of: user, moderator, admin"),
    }
}
