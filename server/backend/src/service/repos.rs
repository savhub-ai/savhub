use std::collections::{HashMap, HashSet};

use chrono::Utc;
use diesel::prelude::*;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use serde_json::json;
use shared::{
    CatalogSource, CreateRepoRequest, FlockDetailResponse, FlockDocument, FlockMetadata,
    FlockRatingStats, FlockSummary, ImportFlockRequest, ImportedSkillMetadata, ImportedSkillRecord,
    PagedResponse, RegistryMaintainer, RegistryStatus, RegistryVisibility, RepoDetailResponse,
    RepoDocument, RepoMetadata, RepoSummary, RuntimeMetadata, validate_flock_document,
    validate_imported_skill_record,
};
use uuid::Uuid;

use super::helpers::{
    db_conn, derive_repo_sign, insert_audit_log, load_users_map, normalize_git_url,
    parse_git_url_parts, sign_to_git_url, user_summary_from_row,
};
use super::interactions::{fetch_flock_comments, get_user_flock_rating, is_flock_starred};
use super::security::parse_security_status;
use crate::auth::{AuthContext, RequestUser};
use crate::error::AppError;
use crate::models::{
    FlockChangeset, FlockRow, NewFlockRow, NewRepoRow, NewSkillRow, RepoRow, SkillChangeset,
    SkillRow, UserRow,
};
use crate::schema::{flocks, repos, skills};

pub async fn list_repos(
    limit: i64,
    cursor: Option<String>,
    q: Option<String>,
) -> Result<PagedResponse<RepoSummary>, AppError> {
    let mut conn = db_conn().await?;
    let limit = limit.clamp(1, 100);
    let offset = cursor
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);

    let search_query = q.filter(|s| !s.trim().is_empty());

    let all_rows = repos::table
        .order(repos::updated_at.desc())
        .select(RepoRow::as_select())
        .load::<RepoRow>(&mut conn)
        .await?;
    let flock_rows = flocks::table
        .filter(flocks::repo_id.eq_any(all_rows.iter().map(|row| row.id).collect::<Vec<_>>()))
        .filter(flocks::soft_deleted_at.is_null())
        .select(FlockRow::as_select())
        .load::<FlockRow>(&mut conn)
        .await?;
    let mut flock_counts = HashMap::new();
    for row in &flock_rows {
        *flock_counts.entry(row.repo_id).or_insert(0i64) += 1;
    }
    let flock_skill_counts =
        load_flock_skill_counts(&mut conn, flock_rows.iter().map(|row| row.id).collect()).await?;
    let repo_skill_counts = load_repo_skill_counts(&flock_rows, &flock_skill_counts);

    if let Some(ref q_str) = search_query {
        let q_lower = q_str.trim().to_lowercase();

        let mut scored: Vec<(i32, &RepoRow)> = all_rows
            .iter()
            .filter_map(|row| {
                let name_lower = row.name.to_lowercase();
                let desc_lower = row.description.to_lowercase();
                let git_url_lower = row.git_url.to_lowercase();

                let matches = git_url_lower.contains(&q_lower)
                    || name_lower.contains(&q_lower)
                    || desc_lower.contains(&q_lower);

                if !matches {
                    return None;
                }

                let score = if git_url_lower.contains(&q_lower) {
                    80
                } else if name_lower == q_lower {
                    70
                } else if name_lower.contains(&q_lower) {
                    50
                } else {
                    10
                };

                Some((score, row))
            })
            .collect();

        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.updated_at.cmp(&a.1.updated_at))
        });

        let total = scored.len();
        let start = (offset as usize).min(total);
        let end = (start + limit as usize).min(total);
        let page = &scored[start..end];

        let next_cursor = if end < total {
            Some((offset + limit).to_string())
        } else {
            None
        };

        let items = page
            .iter()
            .map(|(_, row)| {
                repo_summary_from_row(
                    row,
                    flock_counts.get(&row.id).copied().unwrap_or(0),
                    repo_skill_counts.get(&row.id).copied().unwrap_or(0),
                )
            })
            .collect();

        Ok(PagedResponse { items, next_cursor })
    } else {
        let total = all_rows.len();
        let start = (offset as usize).min(total);
        let end = (start + limit as usize).min(total);
        let has_more = end < total;
        let page = &all_rows[start..end];

        let next_cursor = if has_more {
            Some((offset + limit).to_string())
        } else {
            None
        };

        let items = page
            .iter()
            .map(|row| {
                repo_summary_from_row(
                    row,
                    flock_counts.get(&row.id).copied().unwrap_or(0),
                    repo_skill_counts.get(&row.id).copied().unwrap_or(0),
                )
            })
            .collect();

        Ok(PagedResponse { items, next_cursor })
    }
}

pub async fn create_repo(
    auth: &AuthContext,
    request: CreateRepoRequest,
) -> Result<RepoDetailResponse, AppError> {
    if request.git_url.trim().is_empty() {
        return Err(AppError::BadRequest("git_url is required".to_string()));
    }

    let git_url = normalize_git_url(&request.git_url);
    let (domain, path_slug) = parse_git_url_parts(&git_url);
    if domain.is_empty() || path_slug.is_empty() {
        return Err(AppError::BadRequest(
            "could not parse domain and path from git_url".to_string(),
        ));
    }

    let name = request
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| extract_repo_name(&git_url));
    let description = request
        .description
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_default();

    let mut conn = db_conn().await?;
    if repos::table
        .filter(repos::git_url.eq(&git_url))
        .select(repos::id)
        .first::<Uuid>(&mut conn)
        .await
        .optional()?
        .is_some()
    {
        let sign = format!("{domain}/{path_slug}");
        return Err(AppError::Conflict(format!("repo `{sign}` already exists",)));
    }

    let now = Utc::now();
    let row = NewRepoRow {
        id: Uuid::now_v7(),
        name: name.clone(),
        description: description.clone(),
        git_url: git_url.clone(),
        license: None,
        visibility: "public".to_string(),
        verified: false,
        metadata: serde_json::to_value(RepoMetadata::default())
            .map_err(|error| AppError::Internal(error.to_string()))?,
        keywords: vec![],
        created_at: now,
        updated_at: now,
        last_indexed_at: None,
        git_sha: "main".to_string(),
        git_ref: None,
    };

    diesel::insert_into(repos::table)
        .values(&row)
        .execute(&mut conn)
        .await?;

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "repo.create",
        "repo",
        Some(row.id),
        json!({
            "git_url": row.git_url,
            "name": row.name,
        }),
    )
    .await?;

    get_repo_detail(&domain, &path_slug).await
}

pub async fn get_repo_detail(
    domain: &str,
    path_slug: &str,
) -> Result<RepoDetailResponse, AppError> {
    let mut conn = db_conn().await?;
    let repo_path = format!("{domain}/{path_slug}");
    let git_url = sign_to_git_url(domain, path_slug);
    let repo = repos::table
        .filter(repos::git_url.eq(&git_url))
        .select(RepoRow::as_select())
        .first::<RepoRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("repo `{repo_path}` does not exist")))?;
    let flock_rows = flocks::table
        .filter(flocks::repo_id.eq(repo.id))
        .filter(flocks::soft_deleted_at.is_null())
        .order(flocks::updated_at.desc())
        .select(FlockRow::as_select())
        .load::<FlockRow>(&mut conn)
        .await?;
    let flock_ids = flock_rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let importing_users = load_users_map(
        &mut conn,
        flock_rows
            .iter()
            .map(|row| row.imported_by_user_id)
            .collect(),
    )
    .await?;
    let flock_skill_counts = load_flock_skill_counts(&mut conn, flock_ids.clone()).await?;
    let skill_rows = if flock_ids.is_empty() {
        Vec::new()
    } else {
        skills::table
            .filter(skills::flock_id.eq_any(&flock_ids))
            .filter(skills::soft_deleted_at.is_null())
            .select(SkillRow::as_select())
            .load::<SkillRow>(&mut conn)
            .await?
    };
    let flocks = flock_rows
        .iter()
        .map(|row| {
            flock_summary_from_row(
                row,
                &repo,
                &importing_users,
                flock_skill_counts.get(&row.id).copied().unwrap_or(0),
            )
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let mut skills_list = skill_rows
        .iter()
        .map(imported_skill_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    skills_list.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
    let repo_summary = repo_summary_from_row(
        &repo,
        flocks.len() as i64,
        flock_skill_counts.values().sum(),
    );
    let document = repo_document_from_row(&repo)?;
    Ok(RepoDetailResponse {
        repo: repo_summary,
        document,
        flocks,
        skills: skills_list,
    })
}

pub async fn import_flock(
    auth: &AuthContext,
    domain: &str,
    path_slug: &str,
    request: ImportFlockRequest,
) -> Result<FlockDetailResponse, AppError> {
    let repo = load_import_target(auth, domain, path_slug).await?;
    persist_flock_import(auth, &repo, &request.slug, request.document, request.skills).await
}

pub async fn import_flock_seeded(
    auth: &AuthContext,
    domain: &str,
    path_slug: &str,
    request: ImportFlockRequest,
) -> Result<FlockDetailResponse, AppError> {
    let repo = load_import_target(auth, domain, path_slug).await?;
    persist_flock_import(auth, &repo, &request.slug, request.document, request.skills).await
}

async fn load_import_target(
    _auth: &AuthContext,
    domain: &str,
    path_slug: &str,
) -> Result<RepoRow, AppError> {
    let mut conn = db_conn().await?;
    let repo_path = format!("{domain}/{path_slug}");
    let git_url = sign_to_git_url(domain, path_slug);
    let repo = repos::table
        .filter(repos::git_url.eq(&git_url))
        .select(RepoRow::as_select())
        .first::<RepoRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("repo `{repo_path}` does not exist")))?;

    Ok(repo)
}

async fn persist_flock_import(
    auth: &AuthContext,
    repo: &RepoRow,
    flock_slug: &str,
    mut document: FlockDocument,
    skills: Vec<ImportedSkillRecord>,
) -> Result<FlockDetailResponse, AppError> {
    let mut conn = db_conn().await?;
    let repo_sign_str = derive_repo_sign(&repo.git_url);

    if document.repo != repo.git_url && document.repo != repo_sign_str {
        return Err(AppError::BadRequest(format!(
            "flock repo `{}` does not match path repo `{}`",
            document.repo, repo_sign_str
        )));
    }

    if document.metadata.maintainers.is_empty() {
        document
            .metadata
            .maintainers
            .push(owner_maintainer(auth, "owner"));
    }
    validate_flock_document(&document).map_err(AppError::BadRequest)?;

    let mut seen_skill_slugs = HashSet::new();
    for skill in &skills {
        if !seen_skill_slugs.insert(skill.slug.clone()) {
            return Err(AppError::BadRequest(format!(
                "duplicate flock skill slug `{}`",
                skill.slug
            )));
        }
        validate_imported_skill_record(&repo_sign_str, flock_slug, skill)
            .map_err(AppError::BadRequest)?;
    }

    for featured in &document.metadata.featured_skills {
        if !seen_skill_slugs.contains(featured) {
            return Err(AppError::BadRequest(format!(
                "featured skill `{featured}` is not present in the imported skills list"
            )));
        }
    }

    let existing_flock = flocks::table
        .filter(flocks::repo_id.eq(repo.id))
        .filter(flocks::slug.eq(flock_slug))
        .select(FlockRow::as_select())
        .first::<FlockRow>(&mut conn)
        .await
        .optional()?;

    let now = Utc::now();
    let flock_id = existing_flock
        .as_ref()
        .map(|row| row.id)
        .unwrap_or_else(Uuid::new_v4);
    let flock_metadata = serde_json::to_value(document.metadata.clone())
        .map_err(|error| AppError::Internal(error.to_string()))?;
    let flock_source = document
        .source
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| AppError::Internal(error.to_string()))?
        .unwrap_or(serde_json::Value::Null);
    let flock_row = NewFlockRow {
        id: flock_id,
        repo_id: repo.id,
        slug: flock_slug.to_string(),
        name: document.name.clone(),
        path: String::new(),
        keywords: vec![],
        description: document.description.clone(),
        version: document.version.clone(),
        status: status_to_str(document.status).to_string(),
        visibility: document
            .visibility
            .map(|value| visibility_to_str(value).to_string()),
        license: Some(document.license.clone()),
        metadata: flock_metadata.clone(),
        source: flock_source.clone(),
        imported_by_user_id: auth.user.id,
        created_at: now,
        updated_at: now,
        stats_comments: 0,
        stats_ratings: 0,
        stats_avg_rating: 0.0,
        security_status: "unscanned".to_string(),
        stats_max_installs: 0,
        stats_max_unique_users: 0,
        soft_deleted_at: None,
    };
    let flock_changeset = FlockChangeset {
        name: Some(document.name.clone()),
        path: None,
        keywords: None,
        description: Some(document.description.clone()),
        version: document.version.clone(),
        status: Some(status_to_str(document.status).to_string()),
        visibility: Some(
            document
                .visibility
                .map(|value| visibility_to_str(value).to_string()),
        ),
        license: Some(document.license.clone()),
        metadata: Some(flock_metadata),
        source: Some(flock_source.clone()),
        imported_by_user_id: Some(auth.user.id),
        updated_at: Some(now),
        stats_comments: None,
        stats_ratings: None,
        stats_avg_rating: None,
        security_status: Some("unscanned".to_string()),
        stats_max_installs: None,
        stats_max_unique_users: None,
        soft_deleted_at: None,
    };

    let updated_existing_flock = existing_flock.is_some();
    let result_flock_slug = flock_row.slug.clone();

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            if let Some(existing_flock) = &existing_flock {
                diesel::update(flocks::table.find(existing_flock.id))
                    .set(&flock_changeset)
                    .execute(conn)
                    .await?;

                // Load existing skills indexed by path for upsert matching.
                let existing_skills = skills::table
                    .filter(skills::flock_id.eq(existing_flock.id))
                    .filter(skills::repo_id.eq(repo.id))
                    .select(SkillRow::as_select())
                    .load::<SkillRow>(conn)
                    .await?;
                let existing_by_path: HashMap<String, SkillRow> = existing_skills
                    .into_iter()
                    .map(|row| (row.path.clone(), row))
                    .collect();

                let mut incoming_paths = HashSet::new();
                for skill in &skills {
                    incoming_paths.insert(skill.path.clone());
                    let skill_metadata = serde_json::to_value(skill.metadata.clone())
                        .map_err(|error| AppError::Internal(error.to_string()))?;
                    let skill_runtime = skill
                        .runtime
                        .clone()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(|error| AppError::Internal(error.to_string()))?;

                    if let Some(existing) = existing_by_path.get(&skill.path) {
                        // Update existing skill, preserving stats/stars/comments etc.
                        diesel::update(skills::table.find(existing.id))
                            .set(SkillChangeset {
                                slug: Some(skill.slug.clone()),
                                name: Some(skill.name.clone()),
                                path: Some(skill.path.clone()),
                                description: Some(skill.description.clone()),
                                version: skill.version.clone(),
                                status: Some(status_to_str(skill.status).to_string()),
                                license: Some(skill.license.clone()),
                                source: Some(flock_source.clone()),
                                metadata: Some(skill_metadata),
                                runtime_data: Some(skill_runtime),
                                updated_at: Some(now),
                                ..Default::default()
                            })
                            .execute(conn)
                            .await?;
                    } else {
                        // Insert new skill
                        diesel::insert_into(skills::table)
                            .values(NewSkillRow {
                                id: Uuid::now_v7(),
                                slug: skill.slug.clone(),
                                name: skill.name.clone(),
                                path: skill.path.clone(),
                                keywords: vec![],
                                repo_id: repo.id,
                                flock_id,
                                description: skill.description.clone(),
                                version: skill.version.clone(),
                                status: status_to_str(skill.status).to_string(),
                                license: Some(skill.license.clone()),
                                source: flock_source.clone(),
                                metadata: skill_metadata,
                                entry_data: None,
                                runtime_data: skill_runtime,
                                scan_commit_hash: String::new(),
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
                            })
                            .execute(conn)
                            .await?;
                    }
                }

                // Remove skills whose path no longer exists in the new import
                let removed_ids: Vec<Uuid> = existing_by_path
                    .iter()
                    .filter(|(path, _)| !incoming_paths.contains(path.as_str()))
                    .map(|(_, row)| row.id)
                    .collect();
                if !removed_ids.is_empty() {
                    diesel::delete(skills::table.filter(skills::id.eq_any(&removed_ids)))
                        .execute(conn)
                        .await?;
                }
            } else {
                diesel::insert_into(flocks::table)
                    .values(&flock_row)
                    .execute(conn)
                    .await?;

                let mut skill_rows = Vec::with_capacity(skills.len());
                for skill in &skills {
                    skill_rows.push(NewSkillRow {
                        id: Uuid::now_v7(),
                        slug: skill.slug.clone(),
                        name: skill.name.clone(),
                        path: skill.path.clone(),
                        keywords: vec![],
                        repo_id: repo.id,
                        flock_id,
                        description: skill.description.clone(),
                        version: skill.version.clone(),
                        status: status_to_str(skill.status).to_string(),
                        license: Some(skill.license.clone()),
                        source: flock_source.clone(),
                        metadata: serde_json::to_value(skill.metadata.clone())
                            .map_err(|error| AppError::Internal(error.to_string()))?,
                        entry_data: None,
                        runtime_data: skill
                            .runtime
                            .clone()
                            .map(serde_json::to_value)
                            .transpose()
                            .map_err(|error| AppError::Internal(error.to_string()))?,
                        scan_commit_hash: String::new(),
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
                for chunk in skill_rows.chunks(500) {
                    diesel::insert_into(skills::table)
                        .values(chunk)
                        .execute(conn)
                        .await?;
                }
            }

            insert_audit_log(
                conn,
                Some(auth.user.id),
                "flock.import",
                "flock",
                Some(flock_id),
                json!({
                    "repo": &repo.git_url,
                    "flock_slug": flock_row.slug,
                    "skill_count": skills.len(),
                    "updated": updated_existing_flock,
                }),
            )
            .await?;

            Ok(())
        }
        .scope_boxed()
    })
    .await?;

    let rs = derive_repo_sign(&repo.git_url);
    let (d, ps) = split_repo_path(&rs);
    get_flock_detail(d, ps, &result_flock_slug, None).await
}

pub async fn get_flock_detail(
    domain: &str,
    path_slug: &str,
    flock_slug: &str,
    viewer: Option<&RequestUser>,
) -> Result<FlockDetailResponse, AppError> {
    let mut conn = db_conn().await?;
    let repo_path = format!("{domain}/{path_slug}");
    let git_url = sign_to_git_url(domain, path_slug);
    let repo = repos::table
        .filter(repos::git_url.eq(&git_url))
        .select(RepoRow::as_select())
        .first::<RepoRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("repo `{repo_path}` does not exist")))?;
    let flock = flocks::table
        .filter(flocks::repo_id.eq(repo.id))
        .filter(flocks::slug.eq(flock_slug))
        .select(FlockRow::as_select())
        .first::<FlockRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "flock `{flock_slug}` does not exist under repo `{repo_path}`"
            ))
        })?;
    let users = load_users_map(&mut conn, vec![flock.imported_by_user_id]).await?;
    let skills = skills::table
        .filter(skills::flock_id.eq(flock.id))
        .filter(skills::soft_deleted_at.is_null())
        .order(skills::slug.asc())
        .select(SkillRow::as_select())
        .load::<SkillRow>(&mut conn)
        .await?;
    let document = flock_document_from_row(&repo, &flock)?;
    let comments = fetch_flock_comments(&mut conn, flock.id, viewer).await?;
    let user_rating = match viewer {
        Some(v) => get_user_flock_rating(&mut conn, flock.id, v.id).await?,
        None => None,
    };
    let starred = match viewer {
        Some(v) => is_flock_starred(&mut conn, flock.id, v.id).await,
        None => false,
    };
    Ok(FlockDetailResponse {
        flock: flock_summary_from_row(&flock, &repo, &users, skills.len() as i64)?,
        document,
        skills: skills
            .iter()
            .map(imported_skill_from_row)
            .collect::<Result<Vec<_>, AppError>>()?,
        comments,
        user_rating,
        starred,
    })
}

pub async fn get_flock_by_id(
    flock_id: Uuid,
    viewer: Option<&RequestUser>,
) -> Result<FlockDetailResponse, AppError> {
    let mut conn = db_conn().await?;
    let flock = flocks::table
        .find(flock_id)
        .select(FlockRow::as_select())
        .first::<FlockRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound("flock not found".to_string()))?;
    let repo = repos::table
        .find(flock.repo_id)
        .select(RepoRow::as_select())
        .first::<RepoRow>(&mut conn)
        .await?;
    let users = load_users_map(&mut conn, vec![flock.imported_by_user_id]).await?;
    let skills = skills::table
        .filter(skills::flock_id.eq(flock.id))
        .filter(skills::soft_deleted_at.is_null())
        .order(skills::slug.asc())
        .select(SkillRow::as_select())
        .load::<SkillRow>(&mut conn)
        .await?;
    let document = flock_document_from_row(&repo, &flock)?;
    let comments = fetch_flock_comments(&mut conn, flock.id, viewer).await?;
    let user_rating = match viewer {
        Some(v) => get_user_flock_rating(&mut conn, flock.id, v.id).await?,
        None => None,
    };
    let starred = match viewer {
        Some(v) => is_flock_starred(&mut conn, flock.id, v.id).await,
        None => false,
    };
    Ok(FlockDetailResponse {
        flock: flock_summary_from_row(&flock, &repo, &users, skills.len() as i64)?,
        document,
        skills: skills
            .iter()
            .map(imported_skill_from_row)
            .collect::<Result<Vec<_>, AppError>>()?,
        comments,
        user_rating,
        starred,
    })
}

pub(crate) async fn load_flock_skill_counts(
    conn: &mut AsyncPgConnection,
    flock_ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, i64>, AppError> {
    if flock_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = skills::table
        .filter(skills::flock_id.eq_any(flock_ids))
        .filter(skills::soft_deleted_at.is_null())
        .select(SkillRow::as_select())
        .load::<SkillRow>(conn)
        .await?;
    let mut counts = HashMap::new();
    for row in rows {
        *counts.entry(row.flock_id).or_insert(0) += 1;
    }
    Ok(counts)
}

fn load_repo_skill_counts(
    flock_rows: &[FlockRow],
    flock_skill_counts: &HashMap<Uuid, i64>,
) -> HashMap<Uuid, i64> {
    let mut counts = HashMap::new();
    for row in flock_rows {
        *counts.entry(row.repo_id).or_insert(0) +=
            flock_skill_counts.get(&row.id).copied().unwrap_or(0);
    }
    counts
}

fn repo_summary_from_row(row: &RepoRow, flock_count: i64, skill_count: i64) -> RepoSummary {
    RepoSummary {
        id: row.id,
        name: row.name.clone(),
        description: row.description.clone(),
        git_url: row.git_url.clone(),
        git_sha: Some(row.git_sha.clone()),
        git_branch: row.git_ref.clone(),
        visibility: parse_visibility(&row.visibility),
        verified: row.verified,
        flock_count,
        skill_count,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

pub(crate) fn flock_summary_from_row(
    row: &FlockRow,
    repo: &RepoRow,
    importing_users: &HashMap<Uuid, UserRow>,
    skill_count: i64,
) -> Result<FlockSummary, AppError> {
    let imported_by = importing_users
        .get(&row.imported_by_user_id)
        .ok_or_else(|| AppError::Internal("missing flock importer".to_string()))?;
    Ok(FlockSummary {
        id: row.id,
        repo_url: repo.git_url.clone(),
        slug: row.slug.clone(),
        name: row.name.clone(),
        description: row.description.clone(),
        version: row.version.clone().filter(|v| !v.is_empty()),
        status: parse_status(&row.status),
        visibility: row.visibility.as_deref().map(parse_visibility),
        license: row.license.clone().unwrap_or_default(),
        source: serde_json::from_value::<Option<CatalogSource>>(row.source.clone())
            .map_err(|error| AppError::Internal(error.to_string()))?,
        imported_by: user_summary_from_row(imported_by),
        skill_count,
        rating: FlockRatingStats {
            count: row.stats_ratings,
            average: row.stats_avg_rating,
        },
        stats_comments: row.stats_comments,
        stats_stars: row.stats_stars,
        stats_max_installs: row.stats_max_installs,
        stats_max_unique_users: row.stats_max_unique_users,
        security_status: parse_security_status(&row.security_status),
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn repo_document_from_row(row: &RepoRow) -> Result<RepoDocument, AppError> {
    Ok(RepoDocument {
        name: row.name.clone(),
        description: row.description.clone(),
        git_url: row.git_url.clone(),
        git_sha: Some(row.git_sha.clone()),
        git_branch: row.git_ref.clone(),
        visibility: parse_visibility(&row.visibility),
        verified: row.verified,
        metadata: serde_json::from_value::<RepoMetadata>(row.metadata.clone())
            .map_err(|error| AppError::Internal(error.to_string()))?,
    })
}

fn flock_document_from_row(repo: &RepoRow, row: &FlockRow) -> Result<FlockDocument, AppError> {
    let source: Option<CatalogSource> = serde_json::from_value(row.source.clone())
        .map_err(|error| AppError::Internal(error.to_string()))?;
    let path = source.as_ref().and_then(|s| match s {
        CatalogSource::Registry { path } => Some(path.clone()).filter(|p| p != "."),
    });
    Ok(FlockDocument {
        repo: repo.git_url.clone(),
        name: row.name.clone(),
        description: row.description.clone(),
        path,
        version: row.version.clone().filter(|v| !v.is_empty()),
        status: parse_status(&row.status),
        visibility: row.visibility.as_deref().map(parse_visibility),
        license: row.license.clone().unwrap_or_default(),
        source,
        security: shared::SecuritySummary::default(),
        metadata: serde_json::from_value::<FlockMetadata>(row.metadata.clone())
            .map_err(|error| AppError::Internal(error.to_string()))?,
    })
}

fn imported_skill_from_row(row: &SkillRow) -> Result<ImportedSkillRecord, AppError> {
    Ok(ImportedSkillRecord {
        id: Some(row.id),
        slug: row.slug.clone(),
        path: row.path.clone(),
        name: row.name.clone(),
        description: row.description.clone(),
        version: row.version.clone(),
        status: parse_status(&row.status),
        license: row.license.clone().unwrap_or_default(),
        runtime: row
            .runtime_data
            .clone()
            .map(serde_json::from_value::<RuntimeMetadata>)
            .transpose()
            .map_err(|error| AppError::Internal(error.to_string()))?,
        security: shared::SecuritySummary::default(),
        metadata: serde_json::from_value::<ImportedSkillMetadata>(row.metadata.clone())
            .map_err(|error| AppError::Internal(error.to_string()))?,
    })
}

fn owner_maintainer(auth: &AuthContext, role: &str) -> RegistryMaintainer {
    RegistryMaintainer {
        id: auth.user.handle.clone(),
        name: auth
            .user
            .display_name
            .clone()
            .unwrap_or_else(|| auth.user.handle.clone()),
        role: Some(role.to_string()),
        email: None,
        url: None,
    }
}

fn visibility_to_str(visibility: RegistryVisibility) -> &'static str {
    match visibility {
        RegistryVisibility::Public => "public",
        RegistryVisibility::Unlisted => "unlisted",
        RegistryVisibility::Private => "private",
    }
}

fn parse_visibility(value: &str) -> RegistryVisibility {
    match value {
        "unlisted" => RegistryVisibility::Unlisted,
        "private" => RegistryVisibility::Private,
        _ => RegistryVisibility::Public,
    }
}

fn status_to_str(status: RegistryStatus) -> &'static str {
    match status {
        RegistryStatus::Draft => "draft",
        RegistryStatus::Active => "active",
        RegistryStatus::Experimental => "experimental",
        RegistryStatus::Deprecated => "deprecated",
        RegistryStatus::Archived => "archived",
    }
}

fn parse_status(value: &str) -> RegistryStatus {
    match value {
        "draft" => RegistryStatus::Draft,
        "experimental" => RegistryStatus::Experimental,
        "deprecated" => RegistryStatus::Deprecated,
        "archived" => RegistryStatus::Archived,
        _ => RegistryStatus::Active,
    }
}

/// Split a repo path like `"github.com/owner/name"` into `("github.com", "owner/name")`.
fn split_repo_path(path: &str) -> (&str, &str) {
    path.split_once('/').unwrap_or((path, ""))
}

/// Build a human-readable repo name from the git URL path (domain stripped).
fn extract_repo_name(git_url: &str) -> String {
    let (_domain, path_slug) = parse_git_url_parts(git_url);
    if path_slug.is_empty() {
        return "imported".to_string();
    }
    let name = crate::service::index_jobs::path_to_display_name(&path_slug);
    if name.is_empty() {
        "imported".to_string()
    } else {
        name
    }
}
