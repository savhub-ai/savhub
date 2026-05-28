mod discovery;

use std::collections::{HashMap, HashSet};
use std::fs;

use chrono::Utc;
use diesel::prelude::*;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use discovery::{
    compute_each_dir_as_flock_plans, compute_flock_group_plans, derive_flock_name,
    extract_repo_description, extract_repo_name, join_repo_relative_path,
};
pub(crate) use discovery::{
    flock_name_prefix, format_skill_slug, normalize_skill_name, path_to_display_name,
};
use serde_json::json;
use shared::{
    CatalogSource, FlockMetadata, ImportedSkillRecord, IndexJobDto, IndexJobListResponse,
    IndexJobStatus, RegistryStatus, SubmitIndexRequest, SubmitIndexResponse,
};
use uuid::Uuid;

use super::git_ops::{
    SkillCandidate, apply_repo_redirect, collect_skill_candidates, parse_skill_markdown_metadata,
    refresh_cached_repo, resolve_remote_sha,
};
use super::helpers::{
    db_conn, hash_string, insert_audit_log, normalize_git_url, parse_git_url_parts, take_chars,
};
use crate::auth::AuthContext;
use crate::error::AppError;
use crate::models::{
    FlockChangeset, FlockRow, IndexJobChangeset, IndexJobRow, NewAiRequestCacheRow, NewFlockRow,
    NewIndexJobRow, NewRepoRow, NewSkillRow, RepoRow, SkillChangeset, SkillRow,
};
use crate::schema::{ai_request_cache, flocks, index_jobs, repos, skills};
use crate::state::app_state;

/// Check if we already have a **successful** AI request cached for this
/// (task_type, target_id, commit_sha) combination. Failed requests are
/// ignored so they get retried on the next index.
async fn has_cached_ai_request(
    conn: &mut AsyncPgConnection,
    task_type: &str,
    target_id: Uuid,
    commit_sha: &str,
) -> bool {
    ai_request_cache::table
        .filter(ai_request_cache::task_type.eq(task_type))
        .filter(ai_request_cache::target_id.eq(target_id))
        .filter(ai_request_cache::commit_hash.eq(commit_sha))
        .filter(ai_request_cache::success.eq(true))
        .count()
        .get_result::<i64>(conn)
        .await
        .unwrap_or(0)
        > 0
}

/// Record an AI request result. On conflict (same task+target+commit),
/// update success/error so a retry after a failure overwrites the old record.
async fn cache_ai_request(
    conn: &mut AsyncPgConnection,
    task_type: &str,
    target_type: &str,
    target_id: Uuid,
    commit_sha: &str,
    success: bool,
    error_message: Option<&str>,
) {
    let _ = diesel::insert_into(ai_request_cache::table)
        .values(NewAiRequestCacheRow {
            id: Uuid::now_v7(),
            task_type: task_type.to_string(),
            target_type: target_type.to_string(),
            target_id,
            commit_hash: commit_sha.to_string(),
            success,
            error_message: error_message.map(|s| s.to_string()),
            created_at: Utc::now(),
        })
        .on_conflict((
            ai_request_cache::task_type,
            ai_request_cache::target_id,
            ai_request_cache::commit_hash,
        ))
        .do_update()
        .set((
            ai_request_cache::success.eq(success),
            ai_request_cache::error_message.eq(error_message),
            ai_request_cache::created_at.eq(Utc::now()),
        ))
        .execute(conn)
        .await;
}

/// Check whether the given user is a site admin.
async fn is_site_admin(conn: &mut AsyncPgConnection, user_id: Uuid) -> bool {
    use crate::schema::site_admins;
    site_admins::table
        .filter(site_admins::user_id.eq(user_id))
        .count()
        .get_result::<i64>(conn)
        .await
        .unwrap_or(0)
        > 0
}

/// Find an existing completed scan job with the same git_url and commit_sha.
async fn find_existing_index(
    conn: &mut AsyncPgConnection,
    git_url: &str,
    git_ref: &str,
    git_subdir: &str,
    repo_slug: Option<&str>,
    commit_sha: &str,
) -> Result<Option<IndexJobRow>, AppError> {
    let mut query = index_jobs::table
        .filter(index_jobs::git_url.eq(git_url))
        .filter(index_jobs::git_ref.eq(git_ref))
        .filter(index_jobs::git_subdir.eq(git_subdir))
        .filter(index_jobs::git_sha.eq(commit_sha))
        .filter(index_jobs::status.eq("completed"))
        .order(index_jobs::created_at.desc())
        .into_boxed();

    query = match repo_slug {
        Some(repo_slug) => query.filter(index_jobs::repo_slug.eq(Some(repo_slug.to_string()))),
        None => query.filter(index_jobs::repo_slug.is_null()),
    };

    let row = query
        .select(IndexJobRow::as_select())
        .first::<IndexJobRow>(&mut *conn)
        .await
        .optional()?;
    Ok(row)
}

/// Find an active (pending/running) scan job with the given url_hash.
async fn find_active_index_by_url_hash(
    conn: &mut AsyncPgConnection,
    url_hash: &str,
) -> Result<Option<IndexJobRow>, AppError> {
    let row = index_jobs::table
        .filter(index_jobs::url_hash.eq(url_hash))
        .filter(index_jobs::status.eq_any(["pending", "running"]))
        .order(index_jobs::created_at.desc())
        .select(IndexJobRow::as_select())
        .first::<IndexJobRow>(conn)
        .await
        .optional()?;
    Ok(row)
}

/// Mark an active job as superseded and notify WS subscribers.
pub(crate) async fn supersede_index_job(
    conn: &mut AsyncPgConnection,
    job_id: Uuid,
) -> Result<(), AppError> {
    diesel::update(index_jobs::table.find(job_id))
        .set(IndexJobChangeset {
            status: Some("superseded".to_string()),
            completed_at: Some(Some(Utc::now())),
            updated_at: Some(Utc::now()),
            progress_message: Some("Superseded by newer scan".to_string()),
            ..Default::default()
        })
        .execute(conn)
        .await?;
    let _ = app_state()
        .events_tx
        .send(crate::state::WsEvent::IndexProgress {
            job_id,
            status: "superseded".to_string(),
            progress_pct: 100,
            progress_message: "Superseded by newer scan".to_string(),
            result_data: json!({}),
            error_message: None,
        });
    Ok(())
}

pub async fn submit_index(
    auth: &AuthContext,
    request: SubmitIndexRequest,
) -> Result<SubmitIndexResponse, AppError> {
    if request.git_url.trim().is_empty() {
        return Err(AppError::BadRequest("git_url is required".to_string()));
    }

    // Normalize the git URL so the same repo always produces the same data
    let git_url = normalize_git_url(&request.git_url);
    super::helpers::validate_git_url(&git_url)?;
    let url_hash = hash_string(&git_url);

    // Only admins may set force=true
    let is_admin = {
        let mut conn = db_conn().await?;
        matches!(auth.user.role, shared::UserRole::Admin)
            || is_site_admin(&mut conn, auth.user.id).await
    };
    let force = request.force && is_admin;
    println!(
        "[index] user={} role={:?} request.force={} is_site_admin={} → force={}",
        auth.user.handle, auth.user.role, request.force, is_admin, force
    );

    // Resolve the remote ref to a commit SHA before creating the job
    let commit_sha = resolve_remote_sha(&git_url, &request.git_ref).await?;
    let mut conn = db_conn().await?;

    // Check for an existing completed scan with the same url + sha
    if !force
        && let Some(existing) = find_existing_index(
            &mut conn,
            &git_url,
            &request.git_ref,
            &request.git_subdir,
            request.repo_slug.as_deref(),
            &commit_sha,
        )
        .await?
    {
        println!(
            "[index] skipping duplicate scan for {} @ {} (existing job {})",
            git_url, commit_sha, existing.id
        );
        return Ok(SubmitIndexResponse {
            ok: true,
            job_id: existing.id,
            skipped: true,
            existing_job_id: Some(existing.id),
        });
    }

    // Smart dedup: check for active (pending/running) job with same url_hash
    if !force && let Some(active) = find_active_index_by_url_hash(&mut conn, &url_hash).await? {
        if active.git_sha == commit_sha {
            // Same commit — return existing job
            println!(
                "[index] reusing active job {} for {} @ {}",
                active.id, git_url, commit_sha
            );
            return Ok(SubmitIndexResponse {
                ok: true,
                job_id: active.id,
                skipped: true,
                existing_job_id: Some(active.id),
            });
        } else {
            // Different commit — supersede old job, create new one
            println!(
                "[index] superseding active job {} for {} (old={} new={})",
                active.id, git_url, &active.git_sha, commit_sha
            );
            supersede_index_job(&mut conn, active.id).await?;
        }
    }

    let now = Utc::now();
    let job_id = Uuid::now_v7();

    diesel::insert_into(index_jobs::table)
        .values(NewIndexJobRow {
            id: job_id,
            status: "pending".to_string(),
            job_type: "auto_import".to_string(),
            git_url,
            git_ref: request.git_ref,
            git_subdir: request.git_subdir,
            repo_slug: request.repo_slug,
            requested_by_user_id: auth.user.id,
            result_data: json!({}),
            error_message: None,
            started_at: None,
            completed_at: None,
            created_at: now,
            updated_at: now,
            progress_pct: 0,
            progress_message: "Queued".to_string(),
            git_sha: commit_sha,
            force_index: force,
            url_hash: Some(url_hash),
        })
        .execute(&mut conn)
        .await?;

    Ok(SubmitIndexResponse {
        ok: true,
        job_id,
        skipped: false,
        existing_job_id: None,
    })
}

pub async fn get_index_job(auth: &AuthContext, job_id: Uuid) -> Result<IndexJobDto, AppError> {
    let mut conn = db_conn().await?;
    let row = index_jobs::table
        .find(job_id)
        .select(IndexJobRow::as_select())
        .first::<IndexJobRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound("scan job not found".to_string()))?;

    // Only owner or staff can view
    let is_staff = matches!(
        auth.user.role,
        shared::UserRole::Admin | shared::UserRole::Moderator
    );
    if row.requested_by_user_id != auth.user.id && !is_staff {
        return Err(AppError::Forbidden(
            "you do not have access to this scan job".to_string(),
        ));
    }

    Ok(index_job_dto_from_row(&row))
}

pub async fn list_index_jobs(
    auth: &AuthContext,
    limit: i64,
    offset: i64,
) -> Result<IndexJobListResponse, AppError> {
    let mut conn = db_conn().await?;
    let capped_limit = limit.min(100);
    let rows = index_jobs::table
        .filter(index_jobs::requested_by_user_id.eq(auth.user.id))
        .order(index_jobs::created_at.desc())
        .offset(offset.max(0))
        .limit(capped_limit + 1) // fetch one extra to detect has_more
        .select(IndexJobRow::as_select())
        .load::<IndexJobRow>(&mut conn)
        .await?;

    let has_more = rows.len() as i64 > capped_limit;
    let jobs: Vec<_> = rows
        .iter()
        .take(capped_limit as usize)
        .map(index_job_dto_from_row)
        .collect();

    Ok(IndexJobListResponse { jobs, has_more })
}

pub async fn execute_auto_import(job_id: Uuid) -> Result<(), AppError> {
    let pool = &app_state().async_pool;
    let config = &app_state().config;

    // Mark running
    {
        let mut conn = pool
            .get()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        diesel::update(index_jobs::table.find(job_id))
            .set(IndexJobChangeset {
                status: Some("running".to_string()),
                started_at: Some(Some(Utc::now())),
                updated_at: Some(Utc::now()),
                progress_pct: Some(0),
                progress_message: Some("Starting…".to_string()),
                ..Default::default()
            })
            .execute(&mut conn)
            .await?;
        let _ = app_state()
            .events_tx
            .send(crate::state::WsEvent::IndexProgress {
                job_id,
                status: "running".to_string(),
                progress_pct: 0,
                progress_message: "Starting…".to_string(),
                result_data: serde_json::json!({}),
                error_message: None,
            });
    }

    // Load the job
    let job = {
        let mut conn = pool
            .get()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        index_jobs::table
            .find(job_id)
            .select(IndexJobRow::as_select())
            .first::<IndexJobRow>(&mut conn)
            .await?
    };

    // ── Freshness check ──
    // Before doing real work, verify the remote repo hasn't advanced past the
    // commit this job was created for.  This is especially important for jobs
    // recovered after a server restart: the repo may have received new pushes
    // while the server was down.
    if !job.git_sha.is_empty() {
        let job_sha = &job.git_sha;
        match resolve_remote_sha(&job.git_url, &job.git_ref).await {
            Ok(current_sha) if current_sha != *job_sha => {
                tracing::info!(
                    job_id = %job_id,
                    old_sha = %job_sha,
                    new_sha = %current_sha,
                    "repo has newer commits — superseding and re-queuing"
                );
                let mut conn = pool
                    .get()
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                supersede_index_job(&mut conn, job_id).await?;
                // Create a replacement job targeting the latest commit
                let now = Utc::now();
                let new_job_id = Uuid::now_v7();
                diesel::insert_into(index_jobs::table)
                    .values(NewIndexJobRow {
                        id: new_job_id,
                        status: "pending".to_string(),
                        job_type: job.job_type.clone(),
                        git_url: job.git_url.clone(),
                        git_ref: job.git_ref.clone(),
                        git_subdir: job.git_subdir.clone(),
                        repo_slug: job.repo_slug.clone(),
                        requested_by_user_id: job.requested_by_user_id,
                        result_data: json!({}),
                        error_message: None,
                        started_at: None,
                        completed_at: None,
                        created_at: now,
                        updated_at: now,
                        progress_pct: 0,
                        progress_message: "Queued (superseded stale job)".to_string(),
                        git_sha: current_sha,
                        force_index: job.force_index,
                        url_hash: job.url_hash.clone(),
                    })
                    .execute(&mut conn)
                    .await?;
                tracing::info!(
                    old_job_id = %job_id,
                    new_job_id = %new_job_id,
                    "replacement job created — will be picked up on next tick"
                );
                return Ok(());
            }
            Ok(_) => {
                tracing::debug!(job_id = %job_id, "remote SHA unchanged, proceeding");
            }
            Err(e) => {
                // Network error — proceed with the existing job rather than failing
                tracing::warn!(
                    job_id = %job_id,
                    "could not resolve remote SHA for freshness check: {e} — proceeding anyway"
                );
            }
        }
    }

    match do_auto_import(&job, config).await {
        Ok(result_data) => {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
            diesel::update(index_jobs::table.find(job_id))
                .set(IndexJobChangeset {
                    status: Some("completed".to_string()),
                    result_data: Some(result_data.clone()),
                    completed_at: Some(Some(Utc::now())),
                    updated_at: Some(Utc::now()),
                    progress_pct: Some(100),
                    progress_message: Some("Done".to_string()),
                    ..Default::default()
                })
                .execute(&mut conn)
                .await?;
            let _ = app_state()
                .events_tx
                .send(crate::state::WsEvent::IndexProgress {
                    job_id,
                    status: "completed".to_string(),
                    progress_pct: 100,
                    progress_message: "Done".to_string(),
                    result_data,
                    error_message: None,
                });
        }
        Err(error) => {
            let err_msg = error.to_string();
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
            diesel::update(index_jobs::table.find(job_id))
                .set(IndexJobChangeset {
                    status: Some("failed".to_string()),
                    error_message: Some(Some(err_msg.clone())),
                    completed_at: Some(Some(Utc::now())),
                    updated_at: Some(Utc::now()),
                    progress_pct: Some(100),
                    progress_message: Some(format!("Failed: {}", err_msg)),
                    ..Default::default()
                })
                .execute(&mut conn)
                .await?;
            let _ = app_state()
                .events_tx
                .send(crate::state::WsEvent::IndexProgress {
                    job_id,
                    status: "failed".to_string(),
                    progress_pct: 100,
                    progress_message: format!("Failed: {}", err_msg),
                    result_data: serde_json::json!({}),
                    error_message: Some(err_msg),
                });
        }
    }

    Ok(())
}

async fn do_auto_import(
    job: &IndexJobRow,
    config: &crate::config::Config,
) -> Result<serde_json::Value, AppError> {
    update_progress(job.id, 10, "Preparing repo…").await?;
    let checkout = refresh_cached_repo(
        &config.repo_checkout_base_path(),
        &job.git_url,
        &job.git_ref,
    )
    .await?;
    println!(
        "[index{}] repo ready {} (ref={}) -> {} reused={} commit={}",
        job.id,
        job.git_url,
        job.git_ref,
        checkout.path.display(),
        checkout.reused,
        checkout.head_sha
    );
    // If git followed a redirect, update the stored URL before proceeding.
    // Use the new URL for all subsequent repo_sign / domain / path_slug derivations.
    let effective_git_url = if let Some(ref new_url) = checkout.redirected_url {
        tracing::warn!(
            job_id = %job.id,
            old_url = job.git_url.as_str(),
            new_url = new_url.as_str(),
            "repo has been moved/renamed — applying redirect"
        );
        let normalized = normalize_git_url(new_url);
        // If the repo already exists under the old URL, migrate it in-place
        let mut redir_conn = db_conn().await?;
        let old_git_url = normalize_git_url(&job.git_url);
        if let Some(existing) = repos::table
            .filter(repos::git_url.eq(&old_git_url))
            .select(RepoRow::as_select())
            .first::<RepoRow>(&mut redir_conn)
            .await
            .optional()?
        {
            apply_repo_redirect(&mut redir_conn, existing.id, &job.git_url, new_url).await?;
        }
        normalized
    } else {
        job.git_url.clone()
    };

    // Resolve index rule (strategy + scan path) before determining scan root.
    update_progress(job.id, 20, "Resolving index rules…").await?;
    let repo_name = extract_repo_name(&effective_git_url);
    let (domain, path_slug) = parse_git_url_parts(&effective_git_url);
    let repo_sign = format!("{}/{}", domain, path_slug);
    println!(
        "[index{}] repo_name={}, repo_sign={}",
        job.id, repo_name, repo_sign
    );

    let resolved = {
        let mut rule_conn = app_state()
            .async_pool
            .get()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        super::index_rules::resolve_index_rule(&mut rule_conn, &job.git_url, &job.git_subdir)
            .await?
    };
    println!(
        "[index{}] resolved rule: strategy={:?}, scan_path={}",
        job.id, resolved.strategy, resolved.scan_path
    );

    // Determine scan root from the resolved scan_path
    let effective_subdir = &resolved.scan_path;
    let scan_root = if effective_subdir == "." {
        checkout.path.clone()
    } else {
        checkout.path.join(effective_subdir)
    };
    println!("[index{}] scan_root={}", job.id, scan_root.display());

    if !scan_root.exists() {
        println!("[index{}] scan_root does not exist, aborting", job.id);
        return Err(AppError::BadRequest(format!(
            "subdir `{}` not found in repo",
            effective_subdir
        )));
    }

    // Collect skill candidates
    update_progress(job.id, 30, "Scanning for skills…").await?;
    let mut candidates = Vec::new();
    collect_skill_candidates(&scan_root, ".", &mut candidates)?;
    println!(
        "[index{}] found {} SKILL.md candidates",
        job.id,
        candidates.len()
    );

    for (i, c) in candidates.iter().enumerate() {
        println!(
            "[index{}]   candidate[{}]: relative_dir={:?}, path={}",
            job.id,
            i,
            c.relative_dir,
            c.path.display()
        );
    }

    if candidates.is_empty() {
        return Err(AppError::BadRequest(
            "no SKILL.md files found in repo".to_string(),
        ));
    }

    // ── Incremental detection ──
    // If we reused an existing checkout (pull, not fresh clone), compute which
    // candidate indices actually have file changes so we can skip unchanged
    // flocks entirely.
    let changed_indices: HashSet<usize> = if let (true, Some(prev), false) = (
        checkout.reused,
        checkout.previous_sha.as_deref(),
        job.force_index,
    ) {
        let all_changed =
            super::git_ops::changed_files_between(&checkout.path, prev, &checkout.head_sha)
                .await
                .unwrap_or_default();

        if all_changed.is_empty() {
            // No files changed at all — nothing to do.
            tracing::info!(
                "[index{}] no file changes between {} and {}, skipping",
                job.id,
                &prev[..prev.len().min(8)],
                &checkout.head_sha[..checkout.head_sha.len().min(8)],
            );
            HashSet::new()
        } else {
            tracing::info!(
                "[index{}] incremental: {} files changed between {}..{}",
                job.id,
                all_changed.len(),
                &prev[..prev.len().min(8)],
                &checkout.head_sha[..checkout.head_sha.len().min(8)],
            );
            candidates
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    let prefix = join_repo_relative_path(effective_subdir, &c.relative_dir);
                    all_changed
                        .iter()
                        .any(|f| f.starts_with(&format!("{}/", prefix)) || f == &prefix)
                })
                .map(|(i, _)| i)
                .collect()
        }
    } else {
        // Fresh clone → every candidate is "changed"
        (0..candidates.len()).collect()
    };

    let is_incremental = checkout.reused && checkout.previous_sha.is_some() && !job.force_index;

    // Auto-categorize using the resolved strategy.
    update_progress(job.id, 50, "Categorizing skills…").await?;

    let groups = match resolved.strategy {
        super::index_rules::IndexStrategy::EachDirAsFlock => {
            compute_each_dir_as_flock_plans(&candidates, &repo_name)
        }
        super::index_rules::IndexStrategy::Smart => {
            compute_flock_group_plans(&candidates, &repo_name)
        }
    };
    println!(
        "[index{}] grouping: {:?} → {} flock group(s)",
        job.id,
        resolved.strategy,
        groups.len()
    );
    for group in &groups {
        let source_path = join_repo_relative_path(effective_subdir, &group.source_path);
        println!(
            "[index{}]   group: slug={:?}, source_path={:?}, candidates={}",
            job.id,
            group.slug,
            source_path,
            group.candidate_indices.len()
        );
    }

    let repo_description = extract_repo_description(&checkout.path, &repo_name);
    let repo_description_for_check = repo_description.clone();

    // Ensure repo exists; update name and description on reindex.
    // Acquire a short-lived connection for the repo setup, then release it
    // so the pool is not held during the (potentially long) per-flock loop.
    let repo = {
        let mut conn = db_conn().await?;

        // NOTE: previous flocks/skills are preserved and updated via upsert
        // in persist_auto_import_flock. Stale skills (no longer in the repo)
        // are soft-deleted there. No bulk delete here.

        if let Some(existing) = repos::table
            .filter(repos::git_url.eq(&effective_git_url))
            .select(RepoRow::as_select())
            .first::<RepoRow>(&mut conn)
            .await
            .optional()?
        {
            diesel::update(repos::table.find(existing.id))
                .set(crate::models::RepoChangeset {
                    name: Some(repo_name.to_string()),
                    git_sha: Some(checkout.head_sha.clone()),
                    git_ref: Some(Some(job.git_ref.clone())),
                    updated_at: Some(Utc::now()),
                    ..Default::default()
                })
                .execute(&mut conn)
                .await?;
            RepoRow {
                name: repo_name.to_string(),
                git_sha: checkout.head_sha.clone(),
                git_ref: Some(job.git_ref.clone()),
                ..existing
            }
        } else {
            let now = Utc::now();
            let row = NewRepoRow {
                id: Uuid::now_v7(),
                name: repo_name.to_string(),
                description: repo_description,
                git_url: effective_git_url.clone(),
                license: None,
                visibility: "public".to_string(),
                verified: false,
                metadata: json!({}),
                keywords: vec![],
                created_at: now,
                updated_at: now,
                last_indexed_at: None,
                git_sha: checkout.head_sha.clone(),
                git_ref: Some(job.git_ref.clone()),
            };
            diesel::insert_into(repos::table)
                .values(&row)
                .execute(&mut conn)
                .await?;

            repos::table
                .filter(repos::git_url.eq(&effective_git_url))
                .select(RepoRow::as_select())
                .first::<RepoRow>(&mut conn)
                .await?
        }
    }; // conn is dropped here — returned to the pool

    let mut flock_slugs = Vec::new();
    let mut total_skill_count = 0usize;
    let total_groups = groups.len();

    println!(
        "[index{}] {} flock group(s) to persist",
        job.id, total_groups
    );
    for (group_idx, group) in groups.iter().enumerate() {
        if group.slug.is_empty() {
            continue;
        }

        let flock_slug = group.slug.clone();
        let skill_count = group.candidate_indices.len();

        // ── Incremental fast-path ──
        // If this is an incremental update and none of the candidates in this
        // flock group have changed files, just bump the git sha on the
        // existing skills and skip everything else.
        let flock_has_changes = group
            .candidate_indices
            .iter()
            .any(|i| changed_indices.contains(i));

        if is_incremental && !flock_has_changes {
            let mut conn = db_conn().await?;
            let existing_flock = flocks::table
                .filter(flocks::repo_id.eq(repo.id))
                .filter(flocks::slug.eq(&flock_slug))
                .select(FlockRow::as_select())
                .first::<FlockRow>(&mut conn)
                .await
                .optional()?;

            if let Some(flock_row) = existing_flock {
                // Flock exists and is unchanged — just bump scan_commit_hash.
                // security_status is preserved — no re-scan needed.
                diesel::update(
                    skills::table
                        .filter(skills::flock_id.eq(flock_row.id))
                        .filter(skills::soft_deleted_at.is_null()),
                )
                .set(skills::scan_commit_hash.eq(&checkout.head_sha))
                .execute(&mut conn)
                .await?;

                tracing::debug!(
                    "[index] unchanged flock {}/{} — sha bumped to {}",
                    repo_sign,
                    flock_slug,
                    &checkout.head_sha[..checkout.head_sha.len().min(8)],
                );
                flock_slugs.push(flock_slug);
                total_skill_count += skill_count;
                continue;
            }
            // Flock doesn't exist yet — fall through to full persist
            tracing::info!(
                "[index] flock {}/{} not in DB yet — creating despite no file changes",
                repo_sign,
                flock_slug,
            );
        }

        // Check ai_request_cache to see if flock metadata was already generated
        // for this commit — if so, reuse existing flock row data and skip AI.
        let (existing_flock_for_meta, flock_meta_cached) = {
            let mut conn = db_conn().await?;
            let existing: Option<FlockRow> = flocks::table
                .filter(flocks::repo_id.eq(repo.id))
                .filter(flocks::slug.eq(&flock_slug))
                .select(FlockRow::as_select())
                .first::<FlockRow>(&mut conn)
                .await
                .optional()?;
            let cached = match existing.as_ref() {
                Some(f) => {
                    has_cached_ai_request(&mut conn, "flock_metadata", f.id, &checkout.head_sha)
                        .await
                }
                None => false,
            };
            (existing, cached)
        };

        let default_flock_name = derive_flock_name(&flock_slug, &repo_name);
        let (flock_name, flock_description) = if let (true, Some(f)) =
            (flock_meta_cached, existing_flock_for_meta.as_ref())
        {
            tracing::info!(
                "[index] ai_request_cache hit for flock_metadata {}/{} (commit {})",
                repo_sign,
                flock_slug,
                &checkout.head_sha[..checkout.head_sha.len().min(8)],
            );
            (f.name.clone(), f.description.clone())
        } else if skill_count == 1 {
            let c = &candidates[group.candidate_indices[0]];
            if let Ok(md) = fs::read_to_string(c.path.join("SKILL.md")) {
                if let Ok(meta) = parse_skill_markdown_metadata(&c.relative_dir, &flock_slug, &md) {
                    (meta.name, meta.description)
                } else {
                    (
                        default_flock_name,
                        format!("Auto-imported flock: {flock_slug}"),
                    )
                }
            } else {
                (
                    default_flock_name,
                    format!("Auto-imported flock: {flock_slug}"),
                )
            }
        } else {
            let skill_summaries: Vec<(String, String)> = group
                .candidate_indices
                .iter()
                .filter_map(|&i| {
                    let c = &candidates[i];
                    let md = fs::read_to_string(c.path.join("SKILL.md")).ok()?;
                    let meta =
                        parse_skill_markdown_metadata(&c.relative_dir, &flock_slug, &md).ok()?;
                    Some((meta.name, meta.description))
                })
                .collect();
            let readme_content = ["README.md", "readme.md", "Readme.md", "README"]
                .iter()
                .find_map(|f| fs::read_to_string(checkout.path.join(f)).ok());
            if let Some(ai_meta) =
                super::ai::generate_flock_metadata(&skill_summaries, readme_content.as_deref())
                    .await
            {
                (ai_meta.name, ai_meta.description)
            } else {
                (
                    default_flock_name,
                    format!("Auto-imported flock: {flock_slug}"),
                )
            }
        };

        // Progress: 70..95 spread across flocks
        let pct = 70 + (group_idx * 25) / total_groups.max(1);
        update_progress(
            job.id,
            pct as i32,
            &format!(
                "Persisting flock {}/{} ({}/{})",
                repo_sign,
                flock_slug,
                group_idx + 1,
                total_groups
            ),
        )
        .await?;

        let skills = build_auto_import_skills(
            &candidates,
            &group.candidate_indices,
            &flock_slug,
            &flock_name,
            &job.git_url,
            &job.git_ref,
            effective_subdir,
            &repo_sign,
        )?;

        if skills.is_empty() {
            continue;
        }

        let source_path = join_repo_relative_path(effective_subdir, &group.source_path);

        {
            let mut conn = db_conn().await?;
            persist_auto_import_flock(
                &mut conn,
                &repo,
                &flock_slug,
                &flock_name,
                &flock_description,
                &source_path,
                &checkout.head_sha,
                job.requested_by_user_id,
                &skills,
                "scan_job.auto_import",
            )
            .await?;

            // Record successful flock metadata in ai_request_cache so future
            // re-indexes with the same commit can skip the AI call.
            if !flock_meta_cached
                && let Ok(flock_row) = flocks::table
                    .filter(flocks::repo_id.eq(repo.id))
                    .filter(flocks::slug.eq(&flock_slug))
                    .select(FlockRow::as_select())
                    .first::<FlockRow>(&mut conn)
                    .await
            {
                cache_ai_request(
                    &mut conn,
                    "flock_metadata",
                    "flock",
                    flock_row.id,
                    &checkout.head_sha,
                    true,
                    None,
                )
                .await;
            }
        }

        flock_slugs.push(flock_slug);
        total_skill_count += skill_count;
    }

    // Soft-delete flocks (and their skills) that are no longer present in the repo.
    if !flock_slugs.is_empty() {
        let now = Utc::now();
        let mut conn = db_conn().await?;
        let stale_flock_ids: Vec<Uuid> = flocks::table
            .filter(flocks::repo_id.eq(repo.id))
            .filter(diesel::dsl::not(flocks::slug.eq_any(&flock_slugs)))
            .filter(flocks::soft_deleted_at.is_null())
            .select(flocks::id)
            .load::<Uuid>(&mut conn)
            .await?;
        if !stale_flock_ids.is_empty() {
            tracing::info!(
                "[index{}] soft-deleting {} stale flock(s) for repo {}",
                job.id,
                stale_flock_ids.len(),
                repo.id,
            );
            diesel::update(flocks::table.filter(flocks::id.eq_any(&stale_flock_ids)))
                .set((
                    flocks::soft_deleted_at.eq(Some(now)),
                    flocks::updated_at.eq(now),
                ))
                .execute(&mut conn)
                .await?;
            diesel::update(
                skills::table
                    .filter(skills::flock_id.eq_any(&stale_flock_ids))
                    .filter(skills::soft_deleted_at.is_null()),
            )
            .set((
                skills::soft_deleted_at.eq(Some(now)),
                skills::updated_at.eq(now),
            ))
            .execute(&mut conn)
            .await?;
        }
    }

    // Enhance repo metadata after all flocks are persisted.
    let is_fallback_desc = repo_description_for_check.starts_with("Auto-created repo for ");
    let mut repo_update = crate::models::RepoChangeset {
        updated_at: Some(Utc::now()),
        last_indexed_at: Some(Some(Utc::now())),
        git_sha: Some(checkout.head_sha.clone()),
        git_ref: Some(Some(job.git_ref.clone())),
        ..Default::default()
    };

    // Check ai_request_cache and enhance repo metadata — single conn for sync
    // paths; only the AI branch needs a separate conn after `.await`.
    let needs_ai_repo_meta = {
        let mut conn = db_conn().await?;
        let repo_meta_cached =
            has_cached_ai_request(&mut conn, "repo_metadata", repo.id, &checkout.head_sha).await;

        if repo_meta_cached {
            tracing::info!(
                "[index] ai_request_cache hit for repo_metadata {} (commit {})",
                repo_sign,
                &checkout.head_sha[..checkout.head_sha.len().min(8)],
            );
            false
        } else if flock_slugs.len() == 1 {
            if let Ok(flock_row) = flocks::table
                .filter(flocks::repo_id.eq(repo.id))
                .select(crate::models::FlockRow::as_select())
                .first::<crate::models::FlockRow>(&mut conn)
                .await
            {
                tracing::info!(
                    "[index] single flock — updating repo {} with flock name={:?} desc={:?}",
                    repo_sign,
                    flock_row.name,
                    take_chars(&flock_row.description, 60),
                );
                repo_update.name = Some(flock_row.name.clone());
                repo_update.description = Some(flock_row.description.clone());
                repo_update.keywords = Some(flock_row.keywords.clone());
            }
            cache_ai_request(
                &mut conn,
                "repo_metadata",
                "repo",
                repo.id,
                &checkout.head_sha,
                true,
                None,
            )
            .await;
            false
        } else {
            is_fallback_desc
        }
    }; // conn dropped before potential AI .await

    if needs_ai_repo_meta {
        // Try AI generation from README (needs .await, so conn is acquired
        // only after the async call completes).
        let readme_content = ["README.md", "readme.md", "Readme.md", "README"]
            .iter()
            .find_map(|f| fs::read_to_string(checkout.path.join(f)).ok());
        if let Some(content) = readme_content {
            let (ai_ok, ai_update) = match super::ai::generate_repo_metadata(&content).await {
                Some(ai_meta) => {
                    repo_update.name = Some(ai_meta.name);
                    repo_update.description = Some(ai_meta.description);
                    repo_update.keywords = Some(ai_meta.keywords.into_iter().map(Some).collect());
                    (true, None::<&str>)
                }
                None => (false, Some("AI returned no result")),
            };
            let mut conn = db_conn().await?;
            cache_ai_request(
                &mut conn,
                "repo_metadata",
                "repo",
                repo.id,
                &checkout.head_sha,
                ai_ok,
                ai_update,
            )
            .await;
        }
    }
    {
        let mut conn = db_conn().await?;
        diesel::update(repos::table.find(repo.id))
            .set(&repo_update)
            .execute(&mut conn)
            .await?;
    }

    update_progress(job.id, 95, "Finalizing…").await?;
    println!(
        "[index{}] done: {} flocks, {} skills total. repo cached at {}",
        job.id,
        flock_slugs.len(),
        total_skill_count,
        checkout.path.display()
    );

    Ok(json!({
        "repo": repo_sign,
        "flocks": flock_slugs,
        "skill_count": total_skill_count,
        "commit_sha": checkout.head_sha,
        "previous_commit_sha": checkout.previous_sha,
        "changed_skill_files": checkout.changed_skill_files,
        "repo_path": checkout.path.display().to_string(),
        "reused_checkout": checkout.reused,
    }))
}

pub(crate) fn build_auto_import_skills(
    candidates: &[SkillCandidate],
    indices: &[usize],
    flock_slug: &str,
    flock_name: &str,
    _git_url: &str,
    _git_ref: &str,
    _source_path_root: &str,
    _repo_sign: &str,
) -> Result<Vec<ImportedSkillRecord>, AppError> {
    // First pass: collect normalized skill names and metadata.
    struct Parsed {
        skill_path: String,
        name: String,
        description: String,
        version: Option<String>,
    }
    let mut parsed_skills: Vec<Parsed> = Vec::new();
    for &i in indices {
        let candidate = &candidates[i];
        let skill_path = join_repo_relative_path(_source_path_root, &candidate.relative_dir);
        let markdown = match fs::read_to_string(candidate.path.join("SKILL.md")) {
            Ok(content) => content,
            Err(error) => {
                tracing::warn!("Skipping SKILL.md in {}: {error}", candidate.relative_dir);
                continue;
            }
        };
        let metadata = match parse_skill_markdown_metadata(&skill_path, flock_slug, &markdown) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let name = normalize_skill_name(&metadata.name);
        parsed_skills.push(Parsed {
            skill_path,
            name,
            description: metadata.description,
            version: metadata.version,
        });
    }

    // Decide whether to add a flock-based prefix: if all skill names share
    // the same first word, they already have enough context — skip the prefix.
    let needs_prefix = if parsed_skills.len() <= 1 {
        false
    } else {
        let first_words: HashSet<String> = parsed_skills
            .iter()
            .filter_map(|s| {
                s.name
                    .split_whitespace()
                    .next()
                    .map(|w| w.to_ascii_lowercase())
            })
            .collect();
        first_words.len() != 1
    };

    let prefix = if needs_prefix {
        flock_name_prefix(flock_name)
    } else {
        String::new()
    };

    // Second pass: build the final skill records.
    let mut skills = Vec::new();
    let mut seen_slugs = HashSet::new();
    for parsed in parsed_skills {
        let formatted_name = if prefix.is_empty() {
            parsed.name
        } else {
            format!("{} {}", prefix, parsed.name)
        };
        let slug = format_skill_slug(&formatted_name);

        if slug.is_empty() || !seen_slugs.insert(slug.clone()) {
            continue;
        }

        skills.push(ImportedSkillRecord {
            id: None,
            slug,
            path: parsed.skill_path,
            name: formatted_name,
            description: Some(parsed.description),
            version: parsed.version,
            status: RegistryStatus::Active,
            license: "MIT".to_string(),
            runtime: None,
            security: shared::SecuritySummary::default(),
            metadata: shared::ImportedSkillMetadata::default(),
        });
    }

    skills.sort_by(|left, right| left.slug.cmp(&right.slug));
    Ok(skills)
}

pub(crate) async fn persist_auto_import_flock(
    conn: &mut AsyncPgConnection,
    repo: &RepoRow,
    flock_slug: &str,
    flock_name: &str,
    flock_description: &str,
    source_path: &str,
    commit_sha: &str,
    user_id: Uuid,
    skills: &[ImportedSkillRecord],
    audit_action: &str,
) -> Result<(), AppError> {
    let now = Utc::now();
    let existing_flock = flocks::table
        .filter(flocks::repo_id.eq(repo.id))
        .filter(flocks::slug.eq(flock_slug))
        .select(FlockRow::as_select())
        .first::<FlockRow>(conn)
        .await
        .optional()?;

    let flock_id = existing_flock
        .as_ref()
        .map(|r| r.id)
        .unwrap_or_else(Uuid::new_v4);

    let flock_source = serde_json::to_value(CatalogSource::Registry {
        path: source_path.to_string(),
    })
    .unwrap_or_default();

    let flock_metadata = serde_json::to_value(FlockMetadata::default()).unwrap_or_default();

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            if let Some(existing) = &existing_flock {
                diesel::update(flocks::table.find(existing.id))
                    .set(FlockChangeset {
                        name: Some(flock_name.to_string()),
                        path: Some(source_path.to_string()),
                        description: Some(flock_description.to_string()),
                        version: None,
                        source: Some(flock_source.clone()),
                        metadata: Some(flock_metadata.clone()),
                        updated_at: Some(now),
                        security_status: Some("unscanned".to_string()),
                        soft_deleted_at: Some(None),
                        ..Default::default()
                    })
                    .execute(conn)
                    .await?;
            } else {
                diesel::insert_into(flocks::table)
                    .values(NewFlockRow {
                        id: flock_id,
                        repo_id: repo.id,
                        slug: flock_slug.to_string(),
                        name: flock_name.to_string(),
                        path: source_path.to_string(),
                        keywords: vec![],
                        description: flock_description.to_string(),
                        version: None,
                        status: "active".to_string(),
                        visibility: Some("public".to_string()),
                        license: None,
                        metadata: flock_metadata.clone(),
                        source: flock_source.clone(),
                        imported_by_user_id: user_id,
                        created_at: now,
                        updated_at: now,
                        stats_comments: 0,
                        stats_ratings: 0,
                        stats_avg_rating: 0.0,
                        security_status: "unscanned".to_string(),
                        stats_max_installs: 0,
                        stats_max_unique_users: 0,
                        soft_deleted_at: None,
                    })
                    .execute(conn)
                    .await?;
            }

            let incoming_slugs: Vec<String> = skills.iter().map(|s| s.slug.clone()).collect();
            let existing_skill_rows = skills::table
                .filter(skills::flock_id.eq(flock_id))
                .select(SkillRow::as_select())
                .load::<SkillRow>(conn)
                .await?;
            let existing_by_slug: HashMap<String, SkillRow> = existing_skill_rows
                .into_iter()
                .map(|row| (row.slug.clone(), row))
                .collect();

            let mut new_skill_rows = Vec::new();
            for skill in skills {
                let metadata_json = serde_json::to_value(&skill.metadata).unwrap_or_default();
                let runtime_json = skill
                    .runtime
                    .as_ref()
                    .map(|r| serde_json::to_value(r).unwrap_or_default());

                if let Some(existing) = existing_by_slug.get(&skill.slug) {
                    diesel::update(skills::table.find(existing.id))
                        .set(SkillChangeset {
                            name: Some(skill.name.clone()),
                            path: Some(skill.path.clone()),
                            description: Some(skill.description.clone()),
                            version: skill.version.clone(),
                            license: Some(skill.license.clone()),
                            source: Some(flock_source.clone()),
                            metadata: Some(metadata_json),
                            runtime_data: Some(runtime_json),
                            scan_commit_hash: Some(commit_sha.to_string()),
                            security_status: Some("unscanned".to_string()),
                            soft_deleted_at: Some(None),
                            updated_at: Some(now),
                            ..Default::default()
                        })
                        .execute(conn)
                        .await?;
                } else {
                    new_skill_rows.push(NewSkillRow {
                        id: Uuid::now_v7(),
                        slug: skill.slug.clone(),
                        name: skill.name.clone(),
                        path: skill.path.clone(),
                        keywords: vec![],
                        repo_id: repo.id,
                        flock_id,
                        description: skill.description.clone(),
                        version: skill.version.clone(),
                        status: "active".to_string(),
                        license: Some(skill.license.clone()),
                        source: flock_source.clone(),
                        metadata: metadata_json,
                        entry_data: None,
                        runtime_data: runtime_json,
                        scan_commit_hash: commit_sha.to_string(),
                        security_status: "unscanned".to_string(),
                        latest_version_id: None,
                        tags: serde_json::json!({}),
                        moderation_status: "active".to_string(),
                        highlighted: false,
                        official: false,
                        deprecated: false,
                        suspicious: false,
                        stats_downloads: 0,
                        stats_stars: 0,
                        stats_versions: 0,
                        stats_comments: 0,
                        stats_installs: 0,
                        stats_unique_users: 0,
                        soft_deleted_at: None,
                        created_at: now,
                        updated_at: now,
                    });
                }
            }

            for chunk in new_skill_rows.chunks(200) {
                diesel::insert_into(skills::table)
                    .values(chunk)
                    .execute(conn)
                    .await?;
            }

            if !incoming_slugs.is_empty() {
                diesel::update(
                    skills::table
                        .filter(skills::flock_id.eq(flock_id))
                        .filter(diesel::dsl::not(skills::slug.eq_any(&incoming_slugs)))
                        .filter(skills::soft_deleted_at.is_null()),
                )
                .set((
                    skills::soft_deleted_at.eq(Some(now)),
                    skills::updated_at.eq(now),
                ))
                .execute(conn)
                .await?;
            }

            insert_audit_log(
                conn,
                Some(user_id),
                audit_action,
                "flock",
                Some(flock_id),
                json!({
                    "repo_sign": &repo.git_url,
                    "flock_slug": flock_slug,
                    "skill_count": skills.len(),
                    "source_path": source_path,
                }),
            )
            .await?;

            Ok(())
        }
        .scope_boxed()
    })
    .await?;

    Ok(())
}

fn index_job_dto_from_row(row: &IndexJobRow) -> IndexJobDto {
    IndexJobDto {
        id: row.id,
        status: parse_index_job_status(&row.status),
        job_type: row.job_type.clone(),
        git_url: row.git_url.clone(),
        git_ref: row.git_ref.clone(),
        git_subdir: row.git_subdir.clone(),
        repo_slug: row.repo_slug.clone(),
        result_data: row.result_data.clone(),
        error_message: row.error_message.clone(),
        progress_pct: row.progress_pct,
        progress_message: row.progress_message.clone(),
        started_at: row.started_at,
        completed_at: row.completed_at,
        created_at: row.created_at,
    }
}

async fn update_progress(job_id: Uuid, pct: i32, message: &str) -> Result<(), AppError> {
    let state = app_state();
    let mut conn = state
        .async_pool
        .get()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    diesel::update(index_jobs::table.find(job_id))
        .set(IndexJobChangeset {
            progress_pct: Some(pct),
            progress_message: Some(message.to_string()),
            updated_at: Some(Utc::now()),
            ..Default::default()
        })
        .execute(&mut conn)
        .await?;
    let _ = state.events_tx.send(crate::state::WsEvent::IndexProgress {
        job_id,
        status: "running".to_string(),
        progress_pct: pct,
        progress_message: message.to_string(),
        result_data: serde_json::json!({}),
        error_message: None,
    });
    Ok(())
}

pub(crate) fn parse_index_job_status(value: &str) -> IndexJobStatus {
    match value {
        "running" => IndexJobStatus::Running,
        "completed" => IndexJobStatus::Completed,
        "failed" => IndexJobStatus::Failed,
        "superseded" => IndexJobStatus::Superseded,
        _ => IndexJobStatus::Pending,
    }
}
