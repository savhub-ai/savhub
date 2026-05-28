use std::collections::HashMap;

use diesel::dsl::count_star;
use diesel::prelude::*;
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;
use shared::{
    CommentDto, FileContentResponse, FlockSummary, PagedResponse, ResolveResponse, ResolvedVersion,
    ResourceKind, SearchResponse, SearchResult, SkillDetailResponse, SkillListItem, WhoAmIResponse,
};

use super::helpers::{
    DownloadResult, db_conn, ensure_skill_visible, fetch_owner, fetch_skill_by_path,
    fetch_skill_by_slug, fetch_skill_versions, load_skill_versions_map, load_users_map,
    locate_skill_version, parse_files, resolve_latest_skill_version, score_text,
    skill_item_from_rows, user_summary_from_row, version_detail_from_skill,
    version_summary_from_skill, viewer_is_admin, viewer_is_staff, zip_files,
};
use crate::auth::{AuthContext, RequestUser};
use crate::error::AppError;
use crate::models::{RepoRow, SkillCommentRow, SkillRow, SkillVersionRow};
use crate::schema::{repos, skill_comments, skill_stars, skill_versions, skills};

pub fn whoami(auth: Option<&AuthContext>) -> WhoAmIResponse {
    WhoAmIResponse {
        user: auth.map(|ctx| ctx.user.summary()),
        token_name: auth.map(|ctx| ctx.token_name.clone()),
    }
}

pub async fn list_skills(
    sort: &str,
    limit: i64,
    cursor: Option<String>,
    q: Option<String>,
    repo_id: Option<uuid::Uuid>,
    repo_url: Option<String>,
    path: Option<String>,
    flock_id: Option<uuid::Uuid>,
    viewer: Option<&RequestUser>,
) -> Result<PagedResponse<SkillListItem>, AppError> {
    use crate::schema::skills::dsl;

    let mut conn = db_conn().await?;

    // Precise lookup by repo git_url + path
    if let Some(ref url) = repo_url
        && let Some(ref p) = path
    {
        let row = fetch_skill_by_path(&mut conn, url, p).await?;
        return match row {
            Some(row) => {
                if !viewer_is_staff(viewer) && row.moderation_status != "active" {
                    return Ok(PagedResponse {
                        items: Vec::new(),
                        next_cursor: None,
                    });
                }
                let repo = repos::table
                    .find(row.repo_id)
                    .select(RepoRow::as_select())
                    .first::<RepoRow>(&mut conn)
                    .await?;
                let owners = load_skill_owners(&mut conn, &[&row]).await?;
                let latest = match row.latest_version_id {
                    Some(id) => load_skill_versions_map(&mut conn, vec![id]).await?,
                    None => Default::default(),
                };
                let owner = owners
                    .get(&row.flock_id)
                    .ok_or_else(|| AppError::Internal("missing skill owner".to_string()))?;
                let lv = row.latest_version_id.and_then(|id| latest.get(&id));
                Ok(PagedResponse {
                    items: vec![skill_item_from_rows(&row, &repo.git_url, owner, lv)],
                    next_cursor: None,
                })
            }
            None => Ok(PagedResponse {
                items: Vec::new(),
                next_cursor: None,
            }),
        };
    }

    let limit = limit.clamp(1, 100);
    let offset = cursor
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);

    let search_query = q.filter(|s| !s.trim().is_empty());

    let mut query = dsl::skills.into_boxed();
    query = query.filter(dsl::soft_deleted_at.is_null());
    if !viewer_is_staff(viewer) {
        query = query.filter(dsl::moderation_status.eq("active"));
    }
    if let Some(rid) = repo_id {
        query = query.filter(dsl::repo_id.eq(rid));
    }
    if let Some(fid) = flock_id {
        query = query.filter(dsl::flock_id.eq(fid));
    }

    // Load repo map for all skills (needed to derive sign)
    let repo_map = load_repo_map(&mut conn).await?;

    if search_query.is_some() {
        // When searching, load all rows (no limit/offset) so we can score and sort in-memory.
        let rows = query
            .select(SkillRow::as_select())
            .load::<SkillRow>(&mut conn)
            .await?;

        let q_lower = search_query.as_deref().unwrap().trim().to_lowercase();

        let latest_map = load_skill_versions_map(
            &mut conn,
            rows.iter()
                .filter_map(|row| row.latest_version_id)
                .collect(),
        )
        .await?;

        let mut scored: Vec<(f32, &SkillRow)> = rows
            .iter()
            .filter_map(|row| {
                let search_doc = row
                    .latest_version_id
                    .and_then(|id| latest_map.get(&id))
                    .map(|v| v.search_document.as_str())
                    .unwrap_or_default();
                score_text(
                    &q_lower,
                    &row.slug,
                    &row.name,
                    row.description.as_deref().unwrap_or_default(),
                    search_doc,
                )
                .map(|score| (score, row))
            })
            .collect();

        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
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

        let page_rows: Vec<&SkillRow> = page.iter().map(|(_, row)| *row).collect();
        let owners = load_skill_owners(&mut conn, &page_rows).await?;
        let items = page_rows
            .iter()
            .map(|row| {
                let owner = owners
                    .get(&row.flock_id)
                    .ok_or_else(|| AppError::Internal("missing skill owner".to_string()))?;
                let latest = row.latest_version_id.and_then(|id| latest_map.get(&id));
                let git_url = repo_map
                    .get(&row.repo_id)
                    .map(|r| r.git_url.as_str())
                    .unwrap_or_default();
                Ok(skill_item_from_rows(row, git_url, owner, latest))
            })
            .collect::<Result<Vec<_>, AppError>>()?;

        Ok(PagedResponse { items, next_cursor })
    } else {
        // No search query: use DB-level sorting, limit, and offset.
        query = match sort {
            "downloads" => query.order((dsl::stats_downloads.desc(), dsl::updated_at.desc())),
            "stars" => query.order((dsl::stats_stars.desc(), dsl::updated_at.desc())),
            "installs" => query.order((dsl::stats_installs.desc(), dsl::updated_at.desc())),
            "users" => query.order((dsl::stats_unique_users.desc(), dsl::updated_at.desc())),
            "name" => query.order(dsl::name.asc()),
            _ => query.order(dsl::updated_at.desc()),
        };

        let mut rows = query
            .limit(limit + 1)
            .offset(offset)
            .select(SkillRow::as_select())
            .load::<SkillRow>(&mut conn)
            .await?;

        let next_cursor = if rows.len() > limit as usize {
            rows.pop();
            Some((offset + limit).to_string())
        } else {
            None
        };

        let owners = load_skill_owners(&mut conn, &rows.iter().collect::<Vec<_>>()).await?;
        let latest = load_skill_versions_map(
            &mut conn,
            rows.iter()
                .filter_map(|row| row.latest_version_id)
                .collect(),
        )
        .await?;
        let items = rows
            .iter()
            .map(|row| {
                let owner = owners
                    .get(&row.flock_id)
                    .ok_or_else(|| AppError::Internal("missing skill owner".to_string()))?;
                let latest = row.latest_version_id.and_then(|id| latest.get(&id));
                let git_url = repo_map
                    .get(&row.repo_id)
                    .map(|r| r.git_url.as_str())
                    .unwrap_or_default();
                Ok(skill_item_from_rows(row, git_url, owner, latest))
            })
            .collect::<Result<Vec<_>, AppError>>()?;

        Ok(PagedResponse { items, next_cursor })
    }
}

/// D2: list skills the authenticated user has starred, newest-star first.
///
/// Soft-deleted and moderated-out skills are filtered for non-staff viewers,
/// matching `list_skills`. Limit is clamped to [1, 100].
pub async fn list_my_starred_skills(
    auth: &AuthContext,
    limit: i64,
) -> Result<PagedResponse<SkillListItem>, AppError> {
    use crate::schema::skills::dsl;

    let limit = limit.clamp(1, 100);
    let mut conn = db_conn().await?;

    // Pull starred skill IDs ordered by when they were starred (newest first).
    let starred_ids: Vec<uuid::Uuid> = skill_stars::table
        .filter(skill_stars::user_id.eq(auth.user.id))
        .filter(skill_stars::skill_id.is_not_null())
        .order(skill_stars::created_at.desc())
        .limit(limit)
        .select(skill_stars::skill_id)
        .load::<Option<uuid::Uuid>>(&mut conn)
        .await?
        .into_iter()
        .flatten()
        .collect();

    if starred_ids.is_empty() {
        return Ok(PagedResponse {
            items: Vec::new(),
            next_cursor: None,
        });
    }

    let viewer = Some(&auth.user);
    let mut query = dsl::skills.into_boxed();
    query = query
        .filter(dsl::id.eq_any(&starred_ids))
        .filter(dsl::soft_deleted_at.is_null());
    if !viewer_is_staff(viewer) {
        query = query.filter(dsl::moderation_status.eq("active"));
    }
    let rows: Vec<SkillRow> = query
        .select(SkillRow::as_select())
        .load::<SkillRow>(&mut conn)
        .await?;

    // Preserve the starred-at ordering from the ID query.
    let mut row_by_id: HashMap<uuid::Uuid, SkillRow> =
        rows.into_iter().map(|r| (r.id, r)).collect();
    let ordered_rows: Vec<SkillRow> = starred_ids
        .iter()
        .filter_map(|id| row_by_id.remove(id))
        .collect();

    let repo_map = load_repo_map(&mut conn).await?;
    let owners = load_skill_owners(&mut conn, &ordered_rows.iter().collect::<Vec<_>>()).await?;
    let latest = load_skill_versions_map(
        &mut conn,
        ordered_rows
            .iter()
            .filter_map(|row| row.latest_version_id)
            .collect(),
    )
    .await?;

    let items = ordered_rows
        .iter()
        .map(|row| {
            let owner = owners
                .get(&row.flock_id)
                .ok_or_else(|| AppError::Internal("missing skill owner".to_string()))?;
            let latest = row.latest_version_id.and_then(|id| latest.get(&id));
            let git_url = repo_map
                .get(&row.repo_id)
                .map(|r| r.git_url.as_str())
                .unwrap_or_default();
            Ok(skill_item_from_rows(row, git_url, owner, latest))
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(PagedResponse {
        items,
        next_cursor: None,
    })
}

pub async fn list_flocks(
    sort: &str,
    limit: i64,
    cursor: Option<String>,
    q: Option<String>,
    repo_id: Option<uuid::Uuid>,
    repo_url: Option<String>,
    slug: Option<String>,
) -> Result<PagedResponse<FlockSummary>, AppError> {
    use crate::schema::flocks::dsl;

    let mut conn = db_conn().await?;

    // Precise lookup by repo git_url + flock slug
    if let Some(ref url) = repo_url
        && let Some(ref s) = slug
    {
        let repo = repos::table
            .filter(repos::git_url.eq(url))
            .select(RepoRow::as_select())
            .first::<RepoRow>(&mut conn)
            .await
            .optional()?;
        if let Some(repo) = repo {
            let flock = dsl::flocks
                .filter(dsl::repo_id.eq(repo.id))
                .filter(dsl::slug.eq(s))
                .filter(dsl::soft_deleted_at.is_null())
                .select(crate::models::FlockRow::as_select())
                .first::<crate::models::FlockRow>(&mut conn)
                .await
                .optional()?;
            return match flock {
                Some(flock) => {
                    let items = build_flock_summaries(&mut conn, vec![&flock]).await?;
                    Ok(PagedResponse {
                        items,
                        next_cursor: None,
                    })
                }
                None => Ok(PagedResponse {
                    items: Vec::new(),
                    next_cursor: None,
                }),
            };
        }
        return Ok(PagedResponse {
            items: Vec::new(),
            next_cursor: None,
        });
    }

    let mut conn = db_conn().await?;
    let limit = limit.clamp(1, 100);
    let offset = cursor
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);

    let search_query = q.filter(|s| !s.trim().is_empty());

    if let Some(ref q_str) = search_query {
        let q_lower = q_str.trim().to_lowercase();
        let mut base_query = dsl::flocks
            .into_boxed()
            .filter(dsl::soft_deleted_at.is_null());
        if let Some(rid) = repo_id {
            base_query = base_query.filter(dsl::repo_id.eq(rid));
        }
        let rows = base_query
            .select(crate::models::FlockRow::as_select())
            .load::<crate::models::FlockRow>(&mut conn)
            .await?;

        let mut scored: Vec<(f32, &crate::models::FlockRow)> = rows
            .iter()
            .filter_map(|row| {
                super::helpers::score_text(&q_lower, &row.slug, &row.name, &row.description, "")
                    .map(|score| (score, row))
            })
            .collect();

        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
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

        let items =
            build_flock_summaries(&mut conn, page.iter().map(|(_, row)| *row).collect()).await?;
        Ok(PagedResponse { items, next_cursor })
    } else {
        let mut query = dsl::flocks
            .into_boxed()
            .filter(dsl::soft_deleted_at.is_null());
        if let Some(rid) = repo_id {
            query = query.filter(dsl::repo_id.eq(rid));
        }
        query = match sort {
            "stars" => query.order((dsl::stats_stars.desc(), dsl::updated_at.desc())),
            "installs" => query.order((dsl::stats_max_installs.desc(), dsl::updated_at.desc())),
            "users" => query.order((dsl::stats_max_unique_users.desc(), dsl::updated_at.desc())),
            "name" => query.order(dsl::name.asc()),
            "rating" => query.order((dsl::stats_avg_rating.desc(), dsl::updated_at.desc())),
            _ => query.order(dsl::updated_at.desc()),
        };

        let mut rows = query
            .limit(limit + 1)
            .offset(offset)
            .select(crate::models::FlockRow::as_select())
            .load::<crate::models::FlockRow>(&mut conn)
            .await?;

        let next_cursor = if rows.len() > limit as usize {
            rows.pop();
            Some((offset + limit).to_string())
        } else {
            None
        };

        let items = build_flock_summaries(&mut conn, rows.iter().collect()).await?;
        Ok(PagedResponse { items, next_cursor })
    }
}

async fn build_flock_summaries(
    conn: &mut AsyncPgConnection,
    flock_rows: Vec<&crate::models::FlockRow>,
) -> Result<Vec<FlockSummary>, AppError> {
    use crate::schema::{repos as repo_table, skills as skills_table};

    if flock_rows.is_empty() {
        return Ok(Vec::new());
    }

    // Load repos
    let repo_ids: Vec<uuid::Uuid> = flock_rows
        .iter()
        .map(|r| r.repo_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let repo_rows = repo_table::table
        .filter(repo_table::id.eq_any(&repo_ids))
        .select(RepoRow::as_select())
        .load::<RepoRow>(conn)
        .await?;
    let repo_map: HashMap<uuid::Uuid, RepoRow> = repo_rows.into_iter().map(|r| (r.id, r)).collect();

    // Load importing users
    let user_ids: Vec<uuid::Uuid> = flock_rows.iter().map(|r| r.imported_by_user_id).collect();
    let users = super::helpers::load_users_map(conn, user_ids).await?;

    // Count skills per flock
    let flock_ids: Vec<uuid::Uuid> = flock_rows.iter().map(|r| r.id).collect();
    let skill_counts: Vec<(uuid::Uuid, i64)> = skills_table::table
        .filter(skills_table::flock_id.eq_any(&flock_ids))
        .filter(skills_table::soft_deleted_at.is_null())
        .group_by(skills_table::flock_id)
        .select((skills_table::flock_id, diesel::dsl::count_star()))
        .load::<(uuid::Uuid, i64)>(conn)
        .await?;
    let count_map: HashMap<uuid::Uuid, i64> = skill_counts.into_iter().collect();

    let mut items = Vec::new();
    for row in &flock_rows {
        let Some(repo) = repo_map.get(&row.repo_id) else {
            continue;
        };
        let skill_count = count_map.get(&row.id).copied().unwrap_or(0);
        let summary = super::repos::flock_summary_from_row(row, repo, &users, skill_count)?;
        items.push(summary);
    }
    Ok(items)
}

pub async fn get_skill_detail(
    slug_value: &str,
    viewer: Option<&RequestUser>,
) -> Result<SkillDetailResponse, AppError> {
    let mut conn = db_conn().await?;
    let row = fetch_skill_by_slug(&mut conn, slug_value)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("skill `{slug_value}` does not exist")))?;
    build_skill_detail(&mut conn, row, viewer).await
}

pub async fn get_skill_detail_by_id(
    id: uuid::Uuid,
    viewer: Option<&RequestUser>,
) -> Result<SkillDetailResponse, AppError> {
    let mut conn = db_conn().await?;
    let row = skills::table
        .find(id)
        .select(SkillRow::as_select())
        .first::<SkillRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("skill `{id}` does not exist")))?;
    build_skill_detail(&mut conn, row, viewer).await
}

async fn build_skill_detail(
    conn: &mut AsyncPgConnection,
    row: SkillRow,
    viewer: Option<&RequestUser>,
) -> Result<SkillDetailResponse, AppError> {
    ensure_skill_visible(&row, viewer)?;

    let repo = repos::table
        .find(row.repo_id)
        .select(RepoRow::as_select())
        .first::<RepoRow>(conn)
        .await?;
    let owner = load_skill_owner_via_flock(conn, row.flock_id).await?;
    let versions = fetch_skill_versions(conn, row.id, viewer).await?;
    let latest = resolve_latest_skill_version(&row, &versions);
    let latest_detail = latest.map(version_detail_from_skill).transpose()?;
    let comments = fetch_skill_comments(conn, row.id, viewer).await?;
    let starred = match viewer {
        Some(viewer) => is_skill_starred(conn, row.id, viewer.id).await?,
        None => false,
    };

    Ok(SkillDetailResponse {
        skill: skill_item_from_rows(&row, &repo.git_url, &owner, latest),
        latest_version: latest_detail,
        versions: versions.iter().map(version_summary_from_skill).collect(),
        comments,
        starred,
    })
}

pub async fn get_skill_file(
    slug_value: &str,
    version: Option<&str>,
    tag: Option<&str>,
    path: &str,
    viewer: Option<&RequestUser>,
) -> Result<FileContentResponse, AppError> {
    let mut conn = db_conn().await?;
    let skill = fetch_skill_by_slug(&mut conn, slug_value)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("skill `{slug_value}` does not exist")))?;
    build_skill_file(&mut conn, skill, version, tag, path, viewer).await
}

pub async fn get_skill_file_by_id(
    id: uuid::Uuid,
    version: Option<&str>,
    tag: Option<&str>,
    path: &str,
    viewer: Option<&RequestUser>,
) -> Result<FileContentResponse, AppError> {
    let mut conn = db_conn().await?;
    let skill = skills::table
        .find(id)
        .select(SkillRow::as_select())
        .first::<SkillRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("skill `{id}` does not exist")))?;
    build_skill_file(&mut conn, skill, version, tag, path, viewer).await
}

async fn build_skill_file(
    conn: &mut AsyncPgConnection,
    skill: SkillRow,
    version: Option<&str>,
    tag: Option<&str>,
    path: &str,
    viewer: Option<&RequestUser>,
) -> Result<FileContentResponse, AppError> {
    ensure_skill_visible(&skill, viewer)?;
    let version = locate_skill_version(conn, &skill, version, tag, viewer).await?;
    let files = parse_files(&version.files)?;
    let file = files
        .into_iter()
        .find(|file| file.path == path)
        .ok_or_else(|| AppError::NotFound(format!("file `{path}` not found")))?;
    Ok(FileContentResponse {
        path: file.path,
        content: file.content,
        version: version.version.unwrap_or_default(),
    })
}

pub async fn resolve_skill(
    slug_value: &str,
    fingerprint_value: &str,
) -> Result<ResolveResponse, AppError> {
    let mut conn = db_conn().await?;
    let skill = fetch_skill_by_slug(&mut conn, slug_value)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("skill `{slug_value}` does not exist")))?;
    let matched = skill_versions::table
        .filter(skill_versions::skill_id.eq(skill.id))
        .filter(skill_versions::fingerprint.eq(fingerprint_value))
        .filter(skill_versions::soft_deleted_at.is_null())
        .select(SkillVersionRow::as_select())
        .first::<SkillVersionRow>(&mut conn)
        .await
        .optional()?;
    let latest = match skill.latest_version_id {
        Some(id) => Some(
            skill_versions::table
                .find(id)
                .select(SkillVersionRow::as_select())
                .first::<SkillVersionRow>(&mut conn)
                .await?,
        ),
        None => None,
    };
    Ok(ResolveResponse {
        slug: skill.slug,
        matched: matched.map(|row| ResolvedVersion {
            version: row.version.unwrap_or_default(),
        }),
        latest_version: latest.map(|row| ResolvedVersion {
            version: row.version.unwrap_or_default(),
        }),
    })
}

pub async fn search_catalog(
    query: &str,
    kind: Option<ResourceKind>,
    limit: i64,
) -> Result<SearchResponse, AppError> {
    let mut conn = db_conn().await?;
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(SearchResponse {
            results: Vec::new(),
        });
    }
    let limit = limit.clamp(1, 100) as usize;
    let mut results = Vec::new();

    if kind.is_none() || kind == Some(ResourceKind::Skill) {
        let rows = skills::table
            .filter(skills::soft_deleted_at.is_null())
            .filter(skills::moderation_status.eq("active"))
            .select(SkillRow::as_select())
            .load::<SkillRow>(&mut conn)
            .await?;
        let latest = load_skill_versions_map(
            &mut conn,
            rows.iter()
                .filter_map(|row| row.latest_version_id)
                .collect(),
        )
        .await?;
        let owners = load_skill_owners(&mut conn, &rows.iter().collect::<Vec<_>>()).await?;
        push_skill_results(&rows, &latest, &owners, &query, &mut results);
    }

    results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    results.truncate(limit);
    Ok(SearchResponse { results })
}

pub async fn download_skill_bundle(
    slug_value: &str,
    version: Option<&str>,
    tag: Option<&str>,
    viewer: Option<&RequestUser>,
) -> Result<DownloadResult, AppError> {
    let mut conn = db_conn().await?;
    let skill = fetch_skill_by_slug(&mut conn, slug_value)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("skill `{slug_value}` does not exist")))?;
    ensure_skill_visible(&skill, viewer)?;
    let version = locate_skill_version(&mut conn, &skill, version, tag, viewer).await?;
    let files = parse_files(&version.files)?;
    let bytes = zip_files(&files)?;
    increment_skill_downloads(&mut conn, skill.id).await?;
    Ok(DownloadResult {
        filename: format!(
            "{}-{}.zip",
            skill.slug,
            version.version.as_deref().unwrap_or("0.0.0")
        ),
        content_type: "application/zip".to_string(),
        bytes,
    })
}

async fn fetch_skill_comments(
    conn: &mut AsyncPgConnection,
    skill_id: uuid::Uuid,
    viewer: Option<&RequestUser>,
) -> Result<Vec<CommentDto>, AppError> {
    let can_delete = viewer_is_admin(viewer);
    let rows = skill_comments::table
        .filter(skill_comments::skill_id.eq(skill_id))
        .filter(skill_comments::soft_deleted_at.is_null())
        .order(skill_comments::created_at.asc())
        .select(SkillCommentRow::as_select())
        .load::<SkillCommentRow>(conn)
        .await?;
    let users = load_users_map(conn, rows.iter().map(|row| row.user_id).collect()).await?;
    rows.into_iter()
        .map(|row| {
            let user = users
                .get(&row.user_id)
                .ok_or_else(|| AppError::Internal("missing comment author".to_string()))?;
            Ok(CommentDto {
                id: row.id,
                user: user_summary_from_row(user),
                body: row.body,
                created_at: row.created_at,
                can_delete,
            })
        })
        .collect()
}

async fn is_skill_starred(
    conn: &mut AsyncPgConnection,
    skill_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<bool, AppError> {
    let count = skill_stars::table
        .filter(skill_stars::skill_id.eq(skill_id))
        .filter(skill_stars::user_id.eq(user_id))
        .select(count_star())
        .first::<i64>(conn)
        .await?;
    Ok(count > 0)
}

async fn increment_skill_downloads(
    conn: &mut AsyncPgConnection,
    skill_id: uuid::Uuid,
) -> Result<(), AppError> {
    let row = skills::table
        .find(skill_id)
        .select(SkillRow::as_select())
        .first::<SkillRow>(conn)
        .await?;
    diesel::update(skills::table.find(skill_id))
        .set(skills::stats_downloads.eq(row.stats_downloads + 1))
        .execute(conn)
        .await?;
    Ok(())
}

/// Load the owner (flock importer) UserRow for each skill, keyed by flock_id.
/// Goes through skills -> flocks -> imported_by_user_id -> users.
async fn load_skill_owners(
    conn: &mut AsyncPgConnection,
    skill_rows: &[&SkillRow],
) -> Result<HashMap<uuid::Uuid, crate::models::UserRow>, AppError> {
    use crate::schema::flocks;

    let flock_ids: Vec<uuid::Uuid> = skill_rows
        .iter()
        .map(|row| row.flock_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    if flock_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let flock_rows = flocks::table
        .filter(flocks::id.eq_any(&flock_ids))
        .select(crate::models::FlockRow::as_select())
        .load::<crate::models::FlockRow>(conn)
        .await?;
    let user_ids: Vec<uuid::Uuid> = flock_rows.iter().map(|f| f.imported_by_user_id).collect();
    let users = load_users_map(conn, user_ids).await?;
    let mut result = HashMap::new();
    for flock in flock_rows {
        if let Some(user) = users.get(&flock.imported_by_user_id) {
            result.insert(flock.id, user.clone());
        }
    }
    Ok(result)
}

/// Load the owner (flock importer) for a single skill via its flock_id.
async fn load_skill_owner_via_flock(
    conn: &mut AsyncPgConnection,
    flock_id: uuid::Uuid,
) -> Result<crate::models::UserRow, AppError> {
    use crate::schema::flocks;

    let flock = flocks::table
        .find(flock_id)
        .select(crate::models::FlockRow::as_select())
        .first::<crate::models::FlockRow>(conn)
        .await?;
    fetch_owner(conn, flock.imported_by_user_id).await
}

fn push_skill_results(
    rows: &[SkillRow],
    latest: &HashMap<uuid::Uuid, SkillVersionRow>,
    owners: &HashMap<uuid::Uuid, crate::models::UserRow>,
    query: &str,
    results: &mut Vec<SearchResult>,
) {
    for row in rows {
        let Some(score) = score_text(
            query,
            &row.slug,
            &row.name,
            row.description.as_deref().unwrap_or_default(),
            row.latest_version_id
                .and_then(|id| latest.get(&id))
                .map(|version| version.search_document.as_str())
                .unwrap_or_default(),
        ) else {
            continue;
        };
        results.push(SearchResult {
            kind: ResourceKind::Skill,
            slug: row.slug.clone(),
            display_name: row.name.clone(),
            summary: row.description.clone(),
            score,
            updated_at: row.updated_at,
            latest_version: row
                .latest_version_id
                .and_then(|id| latest.get(&id))
                .map(|version| version.version.clone().unwrap_or_default()),
            owner_handle: owners.get(&row.flock_id).map(|user| user.handle.clone()),
        });
    }
}

/// Load all repos into a map keyed by id.
async fn load_repo_map(
    conn: &mut AsyncPgConnection,
) -> Result<HashMap<uuid::Uuid, RepoRow>, AppError> {
    let rows = repos::table
        .select(RepoRow::as_select())
        .load::<RepoRow>(conn)
        .await?;
    Ok(rows.into_iter().map(|r| (r.id, r)).collect())
}
