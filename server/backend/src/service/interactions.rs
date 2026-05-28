use chrono::Utc;
use diesel::dsl::count_star;
use diesel::prelude::*;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use serde_json::json;
use shared::{
    AdminActionResponse, CommentDto, CreateCommentRequest, FlockRatingStats, RateFlockRequest,
    RateFlockResponse, ToggleStarResponse,
};
use uuid::Uuid;

use super::helpers::{
    db_conn, ensure_skill_visible, fetch_flock_by_path, insert_audit_log, load_users_map,
    user_summary_from_row, viewer_is_admin,
};
use crate::auth::{AuthContext, RequestUser, require_admin};
use crate::error::AppError;
use crate::models::{
    NewSkillCommentRow, NewSkillRatingRow, NewSkillStarRow, SkillCommentRow, SkillRatingRow,
    SkillStarRow,
};
use crate::schema::{flocks, skill_comments, skill_ratings, skill_stars, skill_versions, skills};

pub async fn add_skill_comment(
    auth: &AuthContext,
    skill_id: Uuid,
    request: CreateCommentRequest,
) -> Result<Vec<CommentDto>, AppError> {
    let mut conn = db_conn().await?;
    let skill = skills::table
        .find(skill_id)
        .select(crate::models::SkillRow::as_select())
        .first::<crate::models::SkillRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("skill `{skill_id}` does not exist")))?;
    ensure_skill_visible(&skill, Some(&auth.user))?;
    let body = request.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("comment body is required".to_string()));
    }

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            diesel::insert_into(skill_comments::table)
                .values(NewSkillCommentRow {
                    id: Uuid::now_v7(),
                    skill_id: Some(skill.id),
                    repo_id: skill.repo_id,
                    flock_id: skill.flock_id,
                    user_id: auth.user.id,
                    body: body.to_string(),
                    soft_deleted_at: None,
                    created_at: Utc::now(),
                })
                .execute(conn)
                .await?;
            refresh_skill_stats(conn, skill.id).await?;
            insert_audit_log(
                conn,
                Some(auth.user.id),
                "skill.comment.create",
                "skill",
                Some(skill.id),
                json!({ "skill_id": skill_id }),
            )
            .await?;
            fetch_skill_comments(conn, skill.id, Some(&auth.user)).await
        }
        .scope_boxed()
    })
    .await
}

pub async fn delete_skill_comment(
    auth: &AuthContext,
    skill_id: Uuid,
    comment_id: Uuid,
) -> Result<AdminActionResponse, AppError> {
    require_admin(auth)?;
    let mut conn = db_conn().await?;
    let skill = skills::table
        .find(skill_id)
        .select(crate::models::SkillRow::as_select())
        .first::<crate::models::SkillRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("skill `{skill_id}` does not exist")))?;

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            let deleted = diesel::update(
                skill_comments::table
                    .filter(skill_comments::id.eq(comment_id))
                    .filter(skill_comments::skill_id.eq(skill.id))
                    .filter(skill_comments::soft_deleted_at.is_null()),
            )
            .set(skill_comments::soft_deleted_at.eq(Some(Utc::now())))
            .execute(conn)
            .await?;

            if deleted == 0 {
                return Err(AppError::NotFound("comment not found".to_string()));
            }

            refresh_skill_stats(conn, skill.id).await?;
            insert_audit_log(
                conn,
                Some(auth.user.id),
                "skill.comment.delete",
                "skill_comment",
                Some(comment_id),
                json!({ "skill_id": skill.id }),
            )
            .await?;

            Ok(AdminActionResponse {
                ok: true,
                message: "Comment removed.".to_string(),
            })
        }
        .scope_boxed()
    })
    .await
}

pub async fn toggle_skill_star(
    auth: &AuthContext,
    skill_id: Uuid,
) -> Result<ToggleStarResponse, AppError> {
    let mut conn = db_conn().await?;
    let skill = skills::table
        .find(skill_id)
        .select(crate::models::SkillRow::as_select())
        .first::<crate::models::SkillRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("skill `{skill_id}` does not exist")))?;
    ensure_skill_visible(&skill, Some(&auth.user))?;

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            let existing = skill_stars::table
                .filter(skill_stars::skill_id.eq(skill.id))
                .filter(skill_stars::user_id.eq(auth.user.id))
                .select(SkillStarRow::as_select())
                .first::<SkillStarRow>(conn)
                .await
                .optional()?;

            let starred = if let Some(existing) = existing {
                diesel::delete(skill_stars::table.find(existing.id))
                    .execute(conn)
                    .await?;
                false
            } else {
                diesel::insert_into(skill_stars::table)
                    .values(NewSkillStarRow {
                        id: Uuid::now_v7(),
                        skill_id: Some(skill.id),
                        repo_id: skill.repo_id,
                        flock_id: skill.flock_id,
                        user_id: auth.user.id,
                        created_at: Utc::now(),
                    })
                    .execute(conn)
                    .await?;
                true
            };
            refresh_skill_stats(conn, skill.id).await?;
            Ok(ToggleStarResponse {
                ok: true,
                stars: current_skill_star_count(conn, skill.id).await?,
                starred,
            })
        }
        .scope_boxed()
    })
    .await
}

pub async fn toggle_flock_star(
    auth: &AuthContext,
    repo_domain: &str,
    repo_path_slug: &str,
    flock_slug: &str,
) -> Result<ToggleStarResponse, AppError> {
    let mut conn = db_conn().await?;
    let repo_sign = format!("{repo_domain}/{repo_path_slug}");
    let flock = fetch_flock_by_path(&mut conn, &repo_sign, flock_slug).await?;

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            let existing = skill_stars::table
                .filter(skill_stars::flock_id.eq(flock.id))
                .filter(skill_stars::skill_id.is_null())
                .filter(skill_stars::user_id.eq(auth.user.id))
                .select(SkillStarRow::as_select())
                .first::<SkillStarRow>(conn)
                .await
                .optional()?;

            let starred = if let Some(existing) = existing {
                diesel::delete(skill_stars::table.find(existing.id))
                    .execute(conn)
                    .await?;
                false
            } else {
                diesel::insert_into(skill_stars::table)
                    .values(NewSkillStarRow {
                        id: Uuid::now_v7(),
                        flock_id: flock.id,
                        repo_id: flock.repo_id,
                        skill_id: None,
                        user_id: auth.user.id,
                        created_at: Utc::now(),
                    })
                    .execute(conn)
                    .await?;
                true
            };
            refresh_flock_stats(conn, flock.id).await?;
            Ok(ToggleStarResponse {
                ok: true,
                stars: current_flock_star_count(conn, flock.id).await?,
                starred,
            })
        }
        .scope_boxed()
    })
    .await
}

pub async fn is_flock_starred(conn: &mut AsyncPgConnection, flock_id: Uuid, user_id: Uuid) -> bool {
    skill_stars::table
        .filter(skill_stars::flock_id.eq(flock_id))
        .filter(skill_stars::skill_id.is_null())
        .filter(skill_stars::user_id.eq(user_id))
        .count()
        .get_result::<i64>(conn)
        .await
        .unwrap_or(0)
        > 0
}

/// Return all skill IDs the user has starred.
pub async fn get_starred_skill_ids(auth: &AuthContext) -> Result<Vec<Uuid>, AppError> {
    let mut conn = db_conn().await?;
    let ids: Vec<Option<Uuid>> = skill_stars::table
        .filter(skill_stars::user_id.eq(auth.user.id))
        .filter(skill_stars::skill_id.is_not_null())
        .select(skill_stars::skill_id)
        .load::<Option<Uuid>>(&mut conn)
        .await?;
    Ok(ids.into_iter().flatten().collect())
}

pub async fn fetch_skill_comments(
    conn: &mut AsyncPgConnection,
    skill_id: Uuid,
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

async fn current_skill_star_count(
    conn: &mut AsyncPgConnection,
    skill_id: Uuid,
) -> Result<i64, AppError> {
    skill_stars::table
        .filter(skill_stars::skill_id.eq(skill_id))
        .select(count_star())
        .first::<i64>(conn)
        .await
        .map_err(Into::into)
}

pub async fn refresh_skill_stats(
    conn: &mut AsyncPgConnection,
    skill_id: Uuid,
) -> Result<(), AppError> {
    let stars = current_skill_star_count(conn, skill_id).await?;
    let comments = skill_comments::table
        .filter(skill_comments::skill_id.eq(skill_id))
        .filter(skill_comments::soft_deleted_at.is_null())
        .count()
        .get_result::<i64>(conn)
        .await?;
    let versions = skill_versions::table
        .filter(skill_versions::skill_id.eq(skill_id))
        .filter(skill_versions::soft_deleted_at.is_null())
        .count()
        .get_result::<i64>(conn)
        .await?;
    diesel::update(skills::table.find(skill_id))
        .set((
            skills::stats_stars.eq(stars),
            skills::stats_comments.eq(comments),
            skills::stats_versions.eq(versions),
            skills::updated_at.eq(Utc::now()),
        ))
        .execute(conn)
        .await?;
    Ok(())
}

// --- Flock Comments ---

pub async fn add_flock_comment(
    auth: &AuthContext,
    repo_domain: &str,
    repo_path_slug: &str,
    flock_slug: &str,
    request: CreateCommentRequest,
) -> Result<Vec<CommentDto>, AppError> {
    let mut conn = db_conn().await?;
    let repo_sign = format!("{repo_domain}/{repo_path_slug}");
    let flock = fetch_flock_by_path(&mut conn, &repo_sign, flock_slug).await?;
    let body = request.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("comment body is required".to_string()));
    }

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            diesel::insert_into(skill_comments::table)
                .values(NewSkillCommentRow {
                    id: Uuid::now_v7(),
                    flock_id: flock.id,
                    repo_id: flock.repo_id,
                    skill_id: None,
                    user_id: auth.user.id,
                    body: body.to_string(),
                    soft_deleted_at: None,
                    created_at: Utc::now(),
                })
                .execute(conn)
                .await?;
            refresh_flock_stats(conn, flock.id).await?;
            insert_audit_log(
                conn,
                Some(auth.user.id),
                "flock.comment.create",
                "flock",
                Some(flock.id),
                json!({ "repo": format!("{}/{}", repo_domain, repo_path_slug), "flock_slug": flock_slug }),
            )
            .await?;
            fetch_flock_comments(conn, flock.id, Some(&auth.user)).await
        }
        .scope_boxed()
    })
    .await
}

pub async fn delete_flock_comment(
    auth: &AuthContext,
    repo_domain: &str,
    repo_path_slug: &str,
    flock_slug: &str,
    comment_id: Uuid,
) -> Result<AdminActionResponse, AppError> {
    require_admin(auth)?;
    let mut conn = db_conn().await?;
    let repo_sign = format!("{repo_domain}/{repo_path_slug}");
    let flock = fetch_flock_by_path(&mut conn, &repo_sign, flock_slug).await?;

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            let deleted = diesel::update(
                skill_comments::table
                    .filter(skill_comments::id.eq(comment_id))
                    .filter(skill_comments::flock_id.eq(flock.id))
                    .filter(skill_comments::skill_id.is_null())
                    .filter(skill_comments::soft_deleted_at.is_null()),
            )
            .set(skill_comments::soft_deleted_at.eq(Some(Utc::now())))
            .execute(conn)
            .await?;

            if deleted == 0 {
                return Err(AppError::NotFound("comment not found".to_string()));
            }

            refresh_flock_stats(conn, flock.id).await?;
            insert_audit_log(
                conn,
                Some(auth.user.id),
                "flock.comment.delete",
                "flock_comment",
                Some(comment_id),
                json!({
                    "repo": format!("{}/{}", repo_domain, repo_path_slug),
                    "flock_slug": flock_slug,
                    "flock_id": flock.id
                }),
            )
            .await?;

            Ok(AdminActionResponse {
                ok: true,
                message: "Comment removed.".to_string(),
            })
        }
        .scope_boxed()
    })
    .await
}

pub async fn fetch_flock_comments(
    conn: &mut AsyncPgConnection,
    flock_id: Uuid,
    viewer: Option<&RequestUser>,
) -> Result<Vec<CommentDto>, AppError> {
    let can_delete = viewer_is_admin(viewer);
    let rows = skill_comments::table
        .filter(skill_comments::flock_id.eq(flock_id))
        .filter(skill_comments::skill_id.is_null())
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

// --- Flock Ratings ---

pub async fn rate_flock(
    auth: &AuthContext,
    repo_domain: &str,
    repo_path_slug: &str,
    flock_slug: &str,
    request: RateFlockRequest,
) -> Result<RateFlockResponse, AppError> {
    if request.score < 1 || request.score > 10 {
        return Err(AppError::BadRequest(
            "score must be between 1 and 10".to_string(),
        ));
    }
    let mut conn = db_conn().await?;
    let repo_sign = format!("{repo_domain}/{repo_path_slug}");
    let flock = fetch_flock_by_path(&mut conn, &repo_sign, flock_slug).await?;

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            let existing = skill_ratings::table
                .filter(skill_ratings::flock_id.eq(flock.id))
                .filter(skill_ratings::user_id.eq(auth.user.id))
                .select(SkillRatingRow::as_select())
                .first::<SkillRatingRow>(conn)
                .await
                .optional()?;

            if let Some(existing) = existing {
                diesel::update(skill_ratings::table.find(existing.id))
                    .set((
                        skill_ratings::score.eq(request.score),
                        skill_ratings::updated_at.eq(Utc::now()),
                    ))
                    .execute(conn)
                    .await?;
            } else {
                let now = Utc::now();
                diesel::insert_into(skill_ratings::table)
                    .values(NewSkillRatingRow {
                        id: Uuid::now_v7(),
                        flock_id: flock.id,
                        repo_id: flock.repo_id,
                        user_id: auth.user.id,
                        score: request.score,
                        created_at: now,
                        updated_at: now,
                    })
                    .execute(conn)
                    .await?;
            }

            let stats = compute_flock_rating_stats(conn, flock.id).await?;
            refresh_flock_stats(conn, flock.id).await?;

            insert_audit_log(
                conn,
                Some(auth.user.id),
                "flock.rate",
                "flock",
                Some(flock.id),
                json!({
                    "repo": format!("{}/{}", repo_domain, repo_path_slug),
                    "flock_slug": flock_slug,
                    "score": request.score,
                }),
            )
            .await?;

            Ok(RateFlockResponse {
                ok: true,
                score: request.score,
                stats,
            })
        }
        .scope_boxed()
    })
    .await
}

pub async fn compute_flock_rating_stats(
    conn: &mut AsyncPgConnection,
    flock_id: Uuid,
) -> Result<FlockRatingStats, AppError> {
    let rows = skill_ratings::table
        .filter(skill_ratings::flock_id.eq(flock_id))
        .select(SkillRatingRow::as_select())
        .load::<SkillRatingRow>(conn)
        .await?;
    let count = rows.len() as i64;
    let average = if count > 0 {
        rows.iter().map(|r| r.score as f64).sum::<f64>() / count as f64
    } else {
        0.0
    };
    Ok(FlockRatingStats { count, average })
}

pub async fn get_user_flock_rating(
    conn: &mut AsyncPgConnection,
    flock_id: Uuid,
    user_id: Uuid,
) -> Result<Option<i16>, AppError> {
    skill_ratings::table
        .filter(skill_ratings::flock_id.eq(flock_id))
        .filter(skill_ratings::user_id.eq(user_id))
        .select(skill_ratings::score)
        .first::<i16>(conn)
        .await
        .optional()
        .map_err(Into::into)
}

async fn current_flock_star_count(
    conn: &mut AsyncPgConnection,
    flock_id: Uuid,
) -> Result<i64, AppError> {
    skill_stars::table
        .filter(skill_stars::flock_id.eq(flock_id))
        .filter(skill_stars::skill_id.is_null())
        .select(count_star())
        .first::<i64>(conn)
        .await
        .map_err(Into::into)
}

pub async fn refresh_flock_stats(
    conn: &mut AsyncPgConnection,
    flock_id: Uuid,
) -> Result<(), AppError> {
    let stars = current_flock_star_count(conn, flock_id).await?;
    let comments = skill_comments::table
        .filter(skill_comments::flock_id.eq(flock_id))
        .filter(skill_comments::skill_id.is_null())
        .filter(skill_comments::soft_deleted_at.is_null())
        .count()
        .get_result::<i64>(conn)
        .await?;
    let stats = compute_flock_rating_stats(conn, flock_id).await?;
    diesel::update(flocks::table.find(flock_id))
        .set((
            flocks::stats_stars.eq(stars),
            flocks::stats_comments.eq(comments),
            flocks::stats_ratings.eq(stats.count),
            flocks::stats_avg_rating.eq(stats.average),
            flocks::updated_at.eq(Utc::now()),
        ))
        .execute(conn)
        .await?;
    Ok(())
}

// --- Skill Installs ---

pub async fn record_skill_install(
    repo_url: &str,
    skill_path: &str,
    user_id: Option<Uuid>,
    client_type: &str,
) -> Result<AdminActionResponse, AppError> {
    use crate::schema::{skill_installs, skills as skills_table};

    let mut conn = db_conn().await?;
    let repo = crate::schema::repos::table
        .filter(crate::schema::repos::git_url.eq(repo_url))
        .select(crate::models::RepoRow::as_select())
        .first::<crate::models::RepoRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("repo `{repo_url}` not found")))?;
    let skill = skills_table::table
        .filter(skills_table::repo_id.eq(repo.id))
        .filter(skills_table::path.eq(skill_path))
        .filter(skills_table::soft_deleted_at.is_null())
        .select(crate::models::SkillRow::as_select())
        .first::<crate::models::SkillRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "skill `{skill_path}` not found in repo `{repo_url}`"
            ))
        })?;

    conn.transaction::<_, AppError, _>(|conn| {
        async move {
            diesel::insert_into(skill_installs::table)
                .values(crate::models::NewSkillInstallRow {
                    id: Uuid::now_v7(),
                    skill_id: skill.id,
                    flock_id: skill.flock_id,
                    user_id,
                    client_type: client_type.to_string(),
                    created_at: Utc::now(),
                })
                .execute(conn)
                .await?;
            refresh_skill_install_stats(conn, skill.id).await?;
            refresh_flock_install_stats(conn, skill.flock_id).await?;
            Ok(AdminActionResponse {
                ok: true,
                message: "Install recorded.".to_string(),
            })
        }
        .scope_boxed()
    })
    .await
}

async fn refresh_skill_install_stats(
    conn: &mut AsyncPgConnection,
    skill_id: Uuid,
) -> Result<(), AppError> {
    use crate::schema::skill_installs;

    let total_installs = skill_installs::table
        .filter(skill_installs::skill_id.eq(skill_id))
        .count()
        .get_result::<i64>(conn)
        .await?;
    let unique_users = skill_installs::table
        .filter(skill_installs::skill_id.eq(skill_id))
        .filter(skill_installs::user_id.is_not_null())
        .select(skill_installs::user_id)
        .distinct()
        .count()
        .get_result::<i64>(conn)
        .await?;
    diesel::update(skills::table.find(skill_id))
        .set((
            skills::stats_installs.eq(total_installs),
            skills::stats_unique_users.eq(unique_users),
        ))
        .execute(conn)
        .await?;
    Ok(())
}

async fn refresh_flock_install_stats(
    conn: &mut AsyncPgConnection,
    flock_id: Uuid,
) -> Result<(), AppError> {
    use diesel::dsl::max;

    use crate::schema::skills as skills_table;

    let max_installs: Option<i64> = skills_table::table
        .filter(skills_table::flock_id.eq(flock_id))
        .filter(skills_table::soft_deleted_at.is_null())
        .select(max(skills_table::stats_installs))
        .first(conn)
        .await?;
    let max_unique: Option<i64> = skills_table::table
        .filter(skills_table::flock_id.eq(flock_id))
        .filter(skills_table::soft_deleted_at.is_null())
        .select(max(skills_table::stats_unique_users))
        .first(conn)
        .await?;
    diesel::update(flocks::table.find(flock_id))
        .set((
            flocks::stats_max_installs.eq(max_installs.unwrap_or(0)),
            flocks::stats_max_unique_users.eq(max_unique.unwrap_or(0)),
        ))
        .execute(conn)
        .await?;
    Ok(())
}
