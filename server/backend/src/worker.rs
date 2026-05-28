use std::collections::HashSet;

use chrono::Utc;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use tokio::task::{JoinHandle, JoinSet};
use uuid::Uuid;

use crate::db::AsyncPgPool;
use crate::models::{
    IndexJobRow, NewIndexJobRow, NewPendingIndexRepoRow, PendingIndexRepoRow, RepoRow, SkillRow,
};
use crate::schema::{flocks, index_jobs, pending_index_repos, repos, skills};
use crate::service::git_ops::resolve_remote_sha;
use crate::service::helpers::{hash_string, normalize_git_url};
use crate::state::app_state;

pub fn spawn_worker(pool: AsyncPgPool) -> JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!("background worker started");

        {
            match pool.get().await {
                Ok(mut conn) => {
                    let recovered =
                        diesel::update(index_jobs::table.filter(index_jobs::status.eq("running")))
                            .set(crate::models::IndexJobChangeset {
                                status: Some("pending".to_string()),
                                started_at: Some(None),
                                updated_at: Some(Utc::now()),
                                progress_pct: Some(0),
                                progress_message: Some(
                                    "Queued (recovered after restart)".to_string(),
                                ),
                                ..Default::default()
                            })
                            .execute(&mut conn)
                            .await
                            .unwrap_or(0);

                    if recovered > 0 {
                        tracing::info!("recovered {recovered} stale running job(s) -> pending");
                    }
                }
                Err(e) => {
                    tracing::error!("failed to get DB connection for job recovery: {e}");
                }
            }
        }

        let config = &app_state().config;
        let index_interval = std::time::Duration::from_secs(10);
        let repo_check_interval = std::time::Duration::from_secs(config.sync_interval_secs);

        let mut index_tick = tokio::time::interval(index_interval);
        let mut repo_check_tick = tokio::time::interval(repo_check_interval);
        let mut cleanup_tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        let mut pending_index_tick = tokio::time::interval(std::time::Duration::from_secs(
            config.auto_index_min_interval_secs,
        ));

        // Static security scan — one async task per concurrency slot.
        for i in 0..config.static_scan_concurrency.max(1) {
            let pool = pool.clone();
            tokio::spawn(async move { static_scan_loop(&pool, i).await });
        }

        // AI security scan: same pattern.
        let mut ai_scan_tick = tokio::time::interval(std::time::Duration::from_secs(2));
        let mut ai_scan_tasks: JoinSet<Uuid> = JoinSet::new();
        let max_ai_scan_concurrency = config.ai_security_concurrency.max(1);
        let ai_scan_enabled = config.ai_security_scan_enabled
            && config.ai_provider.is_some()
            && config.ai_api_key.is_some();
        if ai_scan_enabled {
            tracing::info!(
                "[ai-scan] worker enabled (concurrency={})",
                config.ai_security_concurrency
            );
        } else {
            tracing::info!(
                "[ai-scan] worker disabled (ai_security_scan={}, provider={}, key={})",
                config.ai_security_scan_enabled,
                config.ai_provider.is_some(),
                config.ai_api_key.is_some(),
            );
        }
        let mut ai_scanning_ids: HashSet<Uuid> = HashSet::new();

        let mut index_tasks: JoinSet<(Uuid, String)> = JoinSet::new();
        let mut running_url_hashes: HashSet<String> = HashSet::new();

        loop {
            tokio::select! {
                _ = index_tick.tick() => {
                    while let Some(result) = index_tasks.try_join_next() {
                        match result {
                            Ok((job_id, url_hash)) => {
                                running_url_hashes.remove(&url_hash);
                                tracing::debug!(job_id = %job_id, "index task finished");
                            }
                            Err(e) => {
                                tracing::warn!("index task panicked: {e}");
                            }
                        }
                    }

                    let max_jobs = config.max_parallel_index_jobs;
                    if let Err(error) = dispatch_pending_index_jobs(
                        &pool,
                        &mut index_tasks,
                        &mut running_url_hashes,
                        max_jobs,
                    ).await {
                        tracing::warn!("index dispatch error: {error}");
                    }
                }
                _ = repo_check_tick.tick() => {
                    let pool = pool.clone();
                    tokio::spawn(async move {
                        if let Err(error) = check_repos_for_new_commits(&pool).await {
                            tracing::warn!("repo check error: {error}");
                        }
                    });
                }
                _ = ai_scan_tick.tick() => {
                    // Drain completed AI scan tasks
                    while let Some(result) = ai_scan_tasks.try_join_next() {
                        match result {
                            Ok(skill_id) => {
                                ai_scanning_ids.remove(&skill_id);
                            }
                            Err(e) => {
                                tracing::warn!("AI scan task panicked: {e}");
                            }
                        }
                    }

                    if !ai_scan_enabled {
                        continue;
                    }

                    while ai_scan_tasks.len() < max_ai_scan_concurrency {
                        match pick_checked_skill(&pool, &ai_scanning_ids).await {
                            Ok(Some(skill)) => {
                                let skill_id = skill.id;
                                ai_scanning_ids.insert(skill_id);
                                let pool = pool.clone();
                                ai_scan_tasks.spawn(async move {
                                    if let Err(e) =
                                        crate::service::security::process_claimed_ai_scan_task(
                                            &pool, skill,
                                        )
                                        .await
                                    {
                                        tracing::warn!(
                                            skill_id = %skill_id,
                                            "AI scan error: {e}"
                                        );
                                    }
                                    skill_id
                                });
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!("AI scan pick error: {e}");
                                break;
                            }
                        }
                    }
                }
                _ = cleanup_tick.tick() => {
                    match pool.get().await {
                        Ok(mut conn) => {
                            match crate::service::browse_history::cleanup_old_history(&mut conn).await {
                                Ok(n) if n > 0 => tracing::info!("cleaned up {n} old browse history entries"),
                                Ok(_) => {}
                                Err(e) => tracing::warn!("browse history cleanup error: {e}"),
                            }
                        }
                        Err(e) => tracing::warn!("browse history cleanup pool error: {e}"),
                    }
                }
                _ = pending_index_tick.tick() => {
                    let pool = pool.clone();
                    tokio::spawn(async move {
                        if let Err(error) = process_pending_index_repos(&pool).await {
                            tracing::warn!("pending index process error: {error}");
                        }
                    });
                }
                Some(result) = index_tasks.join_next(), if !index_tasks.is_empty() => {
                    match result {
                        Ok((job_id, url_hash)) => {
                            running_url_hashes.remove(&url_hash);
                            tracing::debug!(job_id = %job_id, "index task finished");
                        }
                        Err(e) => {
                            tracing::warn!("index task panicked: {e}");
                        }
                    }
                }
            }
        }
    })
}

async fn dispatch_pending_index_jobs(
    pool: &AsyncPgPool,
    tasks: &mut JoinSet<(Uuid, String)>,
    running_url_hashes: &mut HashSet<String>,
    max_jobs: usize,
) -> Result<(), String> {
    let available_slots = max_jobs.saturating_sub(tasks.len());
    if available_slots == 0 {
        return Ok(());
    }

    let pending_jobs = {
        let mut conn = pool.get().await.map_err(|e| e.to_string())?;
        index_jobs::table
            .filter(index_jobs::status.eq("pending"))
            .order(index_jobs::created_at.asc())
            .limit((available_slots * 3).max(20) as i64)
            .select(IndexJobRow::as_select())
            .load::<IndexJobRow>(&mut conn)
            .await
            .map_err(|e| e.to_string())?
    };

    let mut dispatched = 0usize;
    for job in pending_jobs {
        if dispatched >= available_slots {
            break;
        }

        let url_hash = job
            .url_hash
            .clone()
            .unwrap_or_else(|| hash_string(&normalize_git_url(&job.git_url)));

        if running_url_hashes.contains(&url_hash) {
            continue;
        }

        let job_id = job.id;
        let job_type = job.job_type.clone();
        let uh = url_hash.clone();
        running_url_hashes.insert(url_hash);

        tracing::info!(
            job_id = %job_id,
            job_type = %job_type,
            running = tasks.len() + 1,
            "dispatching index job"
        );

        let pool = pool.clone();
        tasks.spawn(async move {
            execute_index_job(&pool, job_id, &job_type).await;
            (job_id, uh)
        });
        dispatched += 1;
    }

    Ok(())
}

async fn execute_index_job(pool: &AsyncPgPool, job_id: Uuid, job_type: &str) {
    match job_type {
        "auto_import" => {
            if let Err(error) = crate::service::index_jobs::execute_auto_import(job_id).await {
                tracing::error!(job_id = %job_id, "auto_import failed: {error}");
            }
        }
        "resync" => {
            let result = match pool.get().await {
                Ok(mut conn) => diesel::update(index_jobs::table.find(job_id))
                    .set(crate::models::IndexJobChangeset {
                        status: Some("completed".to_string()),
                        completed_at: Some(Some(Utc::now())),
                        updated_at: Some(Utc::now()),
                        ..Default::default()
                    })
                    .execute(&mut conn)
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = result {
                tracing::error!(job_id = %job_id, "resync status update failed: {e}");
            }
        }
        other => {
            tracing::warn!(job_id = %job_id, "unknown job type: {other}");
            let result = match pool.get().await {
                Ok(mut conn) => diesel::update(index_jobs::table.find(job_id))
                    .set(crate::models::IndexJobChangeset {
                        status: Some("failed".to_string()),
                        error_message: Some(Some(format!("unknown job type: {other}"))),
                        completed_at: Some(Some(Utc::now())),
                        updated_at: Some(Utc::now()),
                        ..Default::default()
                    })
                    .execute(&mut conn)
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = result {
                tracing::error!(job_id = %job_id, "failed status update failed: {e}");
            }
        }
    }
}

/// Check ALL repos for new commits and insert changed repos into
/// `pending_index_repos` with `expected_start_at = last_indexed_at + 1 hour`.
/// Repos already in `pending_index_repos` are skipped.
async fn check_repos_for_new_commits(pool: &AsyncPgPool) -> Result<(), String> {
    let config = &app_state().config;
    let interval_secs = config.auto_index_min_interval_secs as i64;

    // Load all repos NOT already in pending_index_repos
    // and NOT indexed within the last interval (default 1 hour)
    let repos_to_check = {
        let mut conn = pool.get().await.map_err(|e| e.to_string())?;
        let threshold = Utc::now() - chrono::Duration::seconds(interval_secs);

        let pending_repo_ids: Vec<Uuid> = pending_index_repos::table
            .select(pending_index_repos::repo_id)
            .load(&mut conn)
            .await
            .map_err(|e| e.to_string())?;

        let mut query = repos::table
            .filter(
                repos::last_indexed_at
                    .is_null()
                    .or(repos::last_indexed_at.lt(threshold)),
            )
            .into_boxed();
        if !pending_repo_ids.is_empty() {
            query = query.filter(diesel::dsl::not(repos::id.eq_any(pending_repo_ids)));
        }
        query
            .select(RepoRow::as_select())
            .load::<RepoRow>(&mut conn)
            .await
            .map_err(|e| e.to_string())?
    };

    if repos_to_check.is_empty() {
        return Ok(());
    }

    tracing::info!(
        count = repos_to_check.len(),
        "checking repos for new commits"
    );

    for repo in repos_to_check {
        let git_url = normalize_git_url(&repo.git_url);
        let git_ref = repo.git_ref.as_deref().unwrap_or("HEAD");

        let current_sha = match resolve_remote_sha(&git_url, git_ref).await {
            Ok(sha) => sha,
            Err(e) => {
                tracing::debug!(repo_id = %repo.id, "failed to ls-remote: {e}");
                continue;
            }
        };

        // No change — skip
        if current_sha == repo.git_sha {
            continue;
        }

        let mut conn = pool.get().await.map_err(|e| e.to_string())?;

        // Already indexed this exact SHA — just update last_indexed_at
        let already_indexed: i64 = index_jobs::table
            .filter(index_jobs::git_url.eq(&git_url))
            .filter(index_jobs::git_sha.eq(&current_sha))
            .filter(index_jobs::status.eq("completed"))
            .count()
            .get_result(&mut conn)
            .await
            .map_err(|e: diesel::result::Error| e.to_string())?;

        if already_indexed > 0 {
            diesel::update(repos::table.find(repo.id))
                .set(crate::models::RepoChangeset {
                    last_indexed_at: Some(Some(Utc::now())),
                    updated_at: Some(Utc::now()),
                    git_sha: Some(current_sha),
                    ..Default::default()
                })
                .execute(&mut conn)
                .await
                .map_err(|e: diesel::result::Error| e.to_string())?;
            continue;
        }

        // Calculate expected_start_at = last_indexed_at + interval, but not before now
        let now = Utc::now();
        let expected_start_at = repo
            .last_indexed_at
            .map(|t| (t + chrono::Duration::seconds(interval_secs)).max(now))
            .unwrap_or(now);

        tracing::info!(
            repo_id = %repo.id,
            git_url = %git_url,
            current_sha = %current_sha,
            expected_start_at = %expected_start_at,
            "repo changed, adding to pending index queue"
        );

        // Insert into pending_index_repos (do nothing on conflict = already queued)
        diesel::insert_into(pending_index_repos::table)
            .values(NewPendingIndexRepoRow {
                id: Uuid::now_v7(),
                repo_id: repo.id,
                expected_start_at,
                created_at: now,
            })
            .on_conflict(pending_index_repos::repo_id)
            .do_nothing()
            .execute(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Process the `pending_index_repos` queue: pick entries whose
/// `expected_start_at <= now`, delete them one by one, create an index job,
/// execute it, and loop until the queue is drained.
async fn process_pending_index_repos(pool: &AsyncPgPool) -> Result<(), String> {
    loop {
        let now = Utc::now();

        // Pick one ready entry
        let entry = {
            let mut conn = pool.get().await.map_err(|e| e.to_string())?;
            pending_index_repos::table
                .filter(pending_index_repos::expected_start_at.le(now))
                .order(pending_index_repos::expected_start_at.asc())
                .select(PendingIndexRepoRow::as_select())
                .first::<PendingIndexRepoRow>(&mut conn)
                .await
                .optional()
                .map_err(|e| e.to_string())?
        };

        let Some(entry) = entry else {
            break;
        };

        // Delete the entry from the queue
        {
            let mut conn = pool.get().await.map_err(|e| e.to_string())?;
            diesel::delete(pending_index_repos::table.find(entry.id))
                .execute(&mut conn)
                .await
                .map_err(|e| e.to_string())?;
        }

        // Load the repo
        let repo = {
            let mut conn = pool.get().await.map_err(|e| e.to_string())?;
            repos::table
                .find(entry.repo_id)
                .select(RepoRow::as_select())
                .first::<RepoRow>(&mut conn)
                .await
                .optional()
                .map_err(|e| e.to_string())?
        };

        let Some(repo) = repo else {
            tracing::warn!(repo_id = %entry.repo_id, "pending index repo not found, skipping");
            continue;
        };

        let git_url = normalize_git_url(&repo.git_url);
        let url_hash = hash_string(&git_url);
        let git_ref = repo.git_ref.as_deref().unwrap_or("HEAD");

        // Resolve current SHA
        let current_sha = match resolve_remote_sha(&git_url, git_ref).await {
            Ok(sha) => sha,
            Err(e) => {
                tracing::warn!(repo_id = %repo.id, "failed to resolve SHA for pending index: {e}");
                continue;
            }
        };

        // Skip if there is already an active job for this URL
        let mut conn = pool.get().await.map_err(|e| e.to_string())?;
        let active_count: i64 = index_jobs::table
            .filter(index_jobs::url_hash.eq(&url_hash))
            .filter(index_jobs::status.eq_any(["pending", "running"]))
            .count()
            .get_result(&mut conn)
            .await
            .map_err(|e: diesel::result::Error| e.to_string())?;

        if active_count > 0 {
            tracing::debug!(repo_id = %repo.id, "skipping pending index, active job exists");
            continue;
        }

        let requested_by = flocks::table
            .filter(flocks::repo_id.eq(repo.id))
            .select(flocks::imported_by_user_id)
            .first::<Uuid>(&mut conn)
            .await
            .unwrap_or(Uuid::nil());

        let now = Utc::now();
        let job_id = Uuid::now_v7();

        tracing::info!(
            repo_id = %repo.id,
            job_id = %job_id,
            git_url = %git_url,
            commit_sha = %current_sha,
            "creating index job from pending queue"
        );

        diesel::insert_into(index_jobs::table)
            .values(NewIndexJobRow {
                id: job_id,
                status: "pending".to_string(),
                job_type: "auto_import".to_string(),
                git_url,
                git_ref: git_ref.to_string(),
                git_subdir: ".".to_string(),
                repo_slug: None,
                requested_by_user_id: requested_by,
                result_data: serde_json::json!({}),
                error_message: None,
                started_at: None,
                completed_at: None,
                created_at: now,
                updated_at: now,
                progress_pct: 0,
                progress_message: "Queued (auto-index)".to_string(),
                git_sha: current_sha,
                force_index: false,
                url_hash: Some(url_hash),
            })
            .execute(&mut conn)
            .await
            .map_err(|e| e.to_string())?;

        // Execute the job and wait for completion
        drop(conn);
        execute_index_job(pool, job_id, "auto_import").await;

        // Update last_indexed_at
        if let Ok(mut conn) = pool.get().await {
            let _ = diesel::update(repos::table.find(repo.id))
                .set(crate::models::RepoChangeset {
                    last_indexed_at: Some(Some(Utc::now())),
                    updated_at: Some(Utc::now()),
                    ..Default::default()
                })
                .execute(&mut conn)
                .await;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Static security scan — single async loop
// ---------------------------------------------------------------------------

/// Continuously pick unscanned skills by ID ascending and run static scans.
///
/// After scanning a skill, the next pick uses `id > last_id`. When no larger
/// ID exists, it wraps around to the smallest unscanned ID. If nothing is
/// unscanned at all, it sleeps for 1 minute before retrying.
async fn static_scan_loop(pool: &AsyncPgPool, thread_idx: usize) {
    tracing::info!("[static-scan-{thread_idx}] loop started");
    let mut last_id: Option<Uuid> = None;

    loop {
        // 1. Try to pick the next unscanned skill with id > last_id
        let skill = pick_next_unscanned_skill(pool, last_id.as_ref()).await;

        // 2. If none found and we had a cursor, wrap around to the beginning
        let skill = match skill {
            Ok(Some(s)) => Some(s),
            Ok(None) if last_id.is_some() => {
                last_id = None;
                match pick_next_unscanned_skill(pool, None).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("[static-scan] pick error (wrap-around): {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                        continue;
                    }
                }
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("[static-scan] pick error: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }
        };

        // 3. Nothing unscanned at all → sleep 1 minute
        let Some(skill) = skill else {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            continue;
        };

        let skill_id = skill.id;
        let slug = skill.slug.clone();

        // 4. Run the scan
        match crate::service::security::run_static_scan_for_skill(pool, &skill).await {
            Ok(true) => {
                tracing::debug!("[static-scan] completed: skill={} ({})", slug, skill_id);
                // Ensure the skill is no longer "unscanned" so we don't pick it again.
                if let Ok(mut conn) = pool.get().await {
                    let current: Option<String> = skills::table
                        .find(skill_id)
                        .select(skills::security_status)
                        .first(&mut conn)
                        .await
                        .ok();
                    if current.as_deref() == Some("unscanned") {
                        let _ = diesel::update(skills::table.find(skill_id))
                            .set(skills::security_status.eq("checked"))
                            .execute(&mut conn)
                            .await;
                    }
                }
            }
            Ok(false) => {
                // No files found — mark as "checked" so it won't be picked again.
                tracing::info!(
                    "[static-scan] no files for skill {} ({}) — marking checked",
                    slug,
                    skill_id,
                );
                if let Ok(mut conn) = pool.get().await {
                    let _ = diesel::update(skills::table.find(skill_id))
                        .set(skills::security_status.eq("checked"))
                        .execute(&mut conn)
                        .await;
                }
            }
            Err(e) => {
                // Error — log it but do NOT mark as checked; it will be
                // retried on the next full pass.
                tracing::error!(
                    "[static-scan] failed for skill {} ({}): {}",
                    slug,
                    skill_id,
                    e,
                );
            }
        }

        // 5. Advance cursor
        last_id = Some(skill_id);
    }
}

/// Pick one unscanned skill with `id > after_id` (or the smallest if `None`),
/// ordered by id ascending.
async fn pick_next_unscanned_skill(
    pool: &AsyncPgPool,
    after_id: Option<&Uuid>,
) -> Result<Option<SkillRow>, String> {
    let mut conn = pool.get().await.map_err(|e| e.to_string())?;

    let mut query = skills::table
        .filter(skills::security_status.eq("unscanned"))
        .filter(skills::soft_deleted_at.is_null())
        .order(skills::id.asc())
        .limit(1)
        .into_boxed();

    if let Some(id) = after_id {
        query = query.filter(skills::id.gt(*id));
    }

    query
        .select(SkillRow::as_select())
        .first::<SkillRow>(&mut conn)
        .await
        .optional()
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// AI security scan: pick statically-checked skills from DB
// ---------------------------------------------------------------------------

/// Pick one skill with `security_status = 'checked'` that is not currently
/// being AI-scanned (not in the in-memory exclusion set).
async fn pick_checked_skill(
    pool: &AsyncPgPool,
    exclude_ids: &HashSet<Uuid>,
) -> Result<Option<SkillRow>, String> {
    let mut conn = pool.get().await.map_err(|e| e.to_string())?;
    let exclude: Vec<Uuid> = exclude_ids.iter().copied().collect();

    let skill = skills::table
        .filter(skills::security_status.eq("checked"))
        .filter(skills::soft_deleted_at.is_null())
        .filter(diesel::dsl::not(skills::id.eq_any(&exclude)))
        .order(skills::updated_at.asc())
        .select(SkillRow::as_select())
        .first::<SkillRow>(&mut conn)
        .await
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(skill)
}
