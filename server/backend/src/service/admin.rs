use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::json;
use shared::{
    AdminActionResponse, AdminIndexJobDto, AdminIndexJobListResponse, AiUsageSummaryItem,
    BanUserRequest, BanUserResponse, CancelIndexJobResponse, CatalogCounts, DeleteResponse,
    IndexJobStatus, ManagementSummaryResponse, ModerationUpdateRequest, RoleUpdateResponse,
    SetUserRoleRequest, UserRole,
};
use uuid::Uuid;

use super::catalog::get_skill_detail_by_id;
use super::helpers::{
    audit_log_entry_from_row, db_conn, insert_audit_log, load_users_map, moderation_status_to_str,
    user_summary_from_row,
};
use crate::auth::{AuthContext, parse_role, require_admin, require_staff};
use crate::error::AppError;
use crate::models::{
    AuditLogRow, IndexJobChangeset, IndexJobRow, SkillChangeset, UserChangeset, UserRow,
};
use crate::schema::{
    ai_usage_logs, audit_logs, flocks, index_jobs, reports, repos, skill_comments, skill_versions,
    skills, user_tokens, users,
};

pub async fn set_skill_deleted(
    auth: &AuthContext,
    skill_id: Uuid,
    deleted: bool,
) -> Result<DeleteResponse, AppError> {
    let mut conn = db_conn().await?;
    let skill = skills::table
        .find(skill_id)
        .select(crate::models::SkillRow::as_select())
        .first::<crate::models::SkillRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("skill `{skill_id}` does not exist")))?;
    if !matches!(auth.user.role, UserRole::Admin | UserRole::Moderator) {
        return Err(AppError::Forbidden(
            "you do not have permission to delete this skill".to_string(),
        ));
    }

    let timestamp = if deleted {
        Some(Some(chrono::Utc::now()))
    } else {
        Some(None)
    };
    let status = if deleted { "removed" } else { "active" };
    diesel::update(skills::table.find(skill.id))
        .set(SkillChangeset {
            moderation_status: Some(status.to_string()),
            soft_deleted_at: timestamp,
            updated_at: Some(chrono::Utc::now()),
            ..Default::default()
        })
        .execute(&mut conn)
        .await?;
    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        if deleted {
            "skill.delete"
        } else {
            "skill.restore"
        },
        "skill",
        Some(skill.id),
        json!({ "skill_id": skill_id }),
    )
    .await?;
    Ok(DeleteResponse { ok: true })
}

pub async fn update_skill_moderation(
    auth: &AuthContext,
    skill_id: Uuid,
    request: ModerationUpdateRequest,
) -> Result<shared::SkillDetailResponse, AppError> {
    if !matches!(auth.user.role, UserRole::Admin | UserRole::Moderator) {
        return Err(AppError::Forbidden(
            "moderator or admin access required".to_string(),
        ));
    }

    let mut conn = db_conn().await?;
    let skill = skills::table
        .find(skill_id)
        .select(crate::models::SkillRow::as_select())
        .first::<crate::models::SkillRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("skill `{skill_id}` does not exist")))?;
    let soft_deleted_at = match request.status {
        shared::ModerationStatus::Removed => Some(Some(chrono::Utc::now())),
        shared::ModerationStatus::Active | shared::ModerationStatus::Hidden => Some(None),
    };

    diesel::update(skills::table.find(skill.id))
        .set(SkillChangeset {
            moderation_status: Some(moderation_status_to_str(request.status).to_string()),
            highlighted: request.highlighted,
            official: request.official,
            deprecated: request.deprecated,
            suspicious: request.suspicious,
            soft_deleted_at,
            updated_at: Some(chrono::Utc::now()),
            ..Default::default()
        })
        .execute(&mut conn)
        .await?;
    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "skill.moderate",
        "skill",
        Some(skill.id),
        json!({
            "status": request.status,
            "highlighted": request.highlighted,
            "official": request.official,
            "deprecated": request.deprecated,
            "suspicious": request.suspicious,
            "notes": request.notes,
        }),
    )
    .await?;
    get_skill_detail_by_id(skill_id, Some(&auth.user)).await
}

pub async fn management_summary(auth: &AuthContext) -> Result<ManagementSummaryResponse, AppError> {
    if !matches!(auth.user.role, UserRole::Admin | UserRole::Moderator) {
        return Err(AppError::Forbidden(
            "moderator or admin access required".to_string(),
        ));
    }
    let mut conn = db_conn().await?;
    let counts = CatalogCounts {
        users: users::table.count().get_result::<i64>(&mut conn).await?,
        repos: repos::table.count().get_result::<i64>(&mut conn).await?,
        flocks: flocks::table
            .filter(flocks::soft_deleted_at.is_null())
            .count()
            .get_result::<i64>(&mut conn)
            .await?,
        skills: skills::table
            .filter(skills::soft_deleted_at.is_null())
            .count()
            .get_result::<i64>(&mut conn)
            .await?,
        versions: skill_versions::table
            .count()
            .get_result::<i64>(&mut conn)
            .await?,
        comments: skill_comments::table
            .count()
            .get_result::<i64>(&mut conn)
            .await?,
        reports: reports::table
            .filter(reports::status.eq("pending"))
            .count()
            .get_result::<i64>(&mut conn)
            .await?,
    };

    let logs = audit_logs::table
        .order(audit_logs::created_at.desc())
        .limit(30)
        .select(AuditLogRow::as_select())
        .load::<AuditLogRow>(&mut conn)
        .await?;
    let actors = load_users_map(
        &mut conn,
        logs.iter().filter_map(|row| row.actor_user_id).collect(),
    )
    .await?;
    let audit_logs = logs
        .into_iter()
        .map(|row| audit_log_entry_from_row(row, &actors))
        .collect();

    // AI usage aggregated by task_type + model
    let ai_rows: Vec<(String, String, i64, i64, i64, i64)> = ai_usage_logs::table
        .group_by((ai_usage_logs::task_type, ai_usage_logs::model))
        .select((
            ai_usage_logs::task_type,
            ai_usage_logs::model,
            diesel::dsl::count(ai_usage_logs::id),
            diesel::dsl::sum(ai_usage_logs::prompt_tokens).assume_not_null(),
            diesel::dsl::sum(ai_usage_logs::completion_tokens).assume_not_null(),
            diesel::dsl::sum(ai_usage_logs::total_tokens).assume_not_null(),
        ))
        .order((ai_usage_logs::task_type.asc(), ai_usage_logs::model.asc()))
        .load(&mut conn)
        .await?;
    let ai_usage = ai_rows
        .into_iter()
        .map(
            |(task_type, model, call_count, prompt, completion, total)| AiUsageSummaryItem {
                task_type,
                model,
                call_count,
                total_prompt_tokens: prompt,
                total_completion_tokens: completion,
                total_tokens: total,
            },
        )
        .collect();

    Ok(ManagementSummaryResponse {
        counts,
        audit_logs,
        ai_usage,
    })
}

pub async fn set_user_role(
    auth: &AuthContext,
    user_id: Uuid,
    request: SetUserRoleRequest,
) -> Result<RoleUpdateResponse, AppError> {
    require_admin(auth)?;
    let mut conn = db_conn().await?;
    let user = users::table
        .find(user_id)
        .select(UserRow::as_select())
        .first::<UserRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("user `{user_id}` not found")))?;
    if auth.user.id == user.id && request.role != UserRole::Admin {
        return Err(AppError::Conflict(
            "you cannot remove your own admin role".to_string(),
        ));
    }

    diesel::update(users::table.find(user.id))
        .set(UserChangeset {
            handle: None,
            display_name: None,
            bio: None,
            avatar_url: None,
            github_user_id: None,
            github_login: None,
            role: Some(user_role_to_str(request.role).to_string()),
            updated_at: Some(chrono::Utc::now()),
        })
        .execute(&mut conn)
        .await?;

    let updated = users::table
        .find(user.id)
        .select(UserRow::as_select())
        .first::<UserRow>(&mut conn)
        .await?;
    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "user.role.set",
        "user",
        Some(updated.id),
        json!({
            "handle": updated.handle,
            "role": request.role,
        }),
    )
    .await?;
    Ok(RoleUpdateResponse {
        ok: true,
        user: user_summary_from_row(&updated),
    })
}

pub async fn ban_user(
    auth: &AuthContext,
    user_id: Uuid,
    request: BanUserRequest,
) -> Result<BanUserResponse, AppError> {
    require_staff(auth)?;
    let mut conn = db_conn().await?;
    let user = users::table
        .find(user_id)
        .select(UserRow::as_select())
        .first::<UserRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("user `{user_id}` not found")))?;
    if auth.user.id == user.id {
        return Err(AppError::Conflict("you cannot ban yourself".to_string()));
    }
    if parse_role(&user.role) == UserRole::Admin && auth.user.role != UserRole::Admin {
        return Err(AppError::Forbidden(
            "only an admin may ban another admin".to_string(),
        ));
    }

    let now = chrono::Utc::now();
    let revoked_tokens = diesel::delete(user_tokens::table.filter(user_tokens::user_id.eq(user.id)))
        .execute(&mut conn)
        .await? as i64;
    // Find flocks imported by the banned user, then soft-delete their skills
    let user_flock_ids: Vec<Uuid> = flocks::table
        .filter(flocks::imported_by_user_id.eq(user.id))
        .select(flocks::id)
        .load::<Uuid>(&mut conn)
        .await?;
    let deleted_skills = if user_flock_ids.is_empty() {
        0i64
    } else {
        diesel::update(
            skills::table
                .filter(skills::flock_id.eq_any(&user_flock_ids))
                .filter(skills::soft_deleted_at.is_null()),
        )
        .set((
            skills::moderation_status.eq("removed"),
            skills::soft_deleted_at.eq(Some(now)),
            skills::updated_at.eq(now),
        ))
        .execute(&mut conn)
        .await? as i64
    };
    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "user.ban",
        "user",
        Some(user.id),
        json!({
            "handle": user.handle,
            "reason": request.reason,
            "revoked_tokens": revoked_tokens,
            "deleted_skills": deleted_skills,
        }),
    )
    .await?;
    Ok(BanUserResponse {
        ok: true,
        user: user_summary_from_row(&user),
        revoked_tokens,
        deleted_skills,
    })
}

pub async fn list_all_index_jobs(
    auth: &AuthContext,
    q: Option<&str>,
    limit: i64,
    cursor: Option<String>,
) -> Result<AdminIndexJobListResponse, AppError> {
    require_staff(auth)?;
    let mut conn = db_conn().await?;

    let offset: i64 = cursor
        .as_deref()
        .and_then(|c| c.parse::<i64>().ok())
        .unwrap_or(0);
    let limit = limit.clamp(1, 100);

    let rows = index_jobs::table
        .order(index_jobs::created_at.desc())
        .limit(500)
        .select(IndexJobRow::as_select())
        .load::<IndexJobRow>(&mut conn)
        .await?;

    let user_ids: Vec<Uuid> = rows.iter().map(|r| r.requested_by_user_id).collect();
    let users_map = load_users_map(&mut conn, user_ids).await?;

    let mut dtos: Vec<AdminIndexJobDto> = rows
        .iter()
        .filter_map(|row| {
            let user = users_map.get(&row.requested_by_user_id)?;
            Some(AdminIndexJobDto {
                id: row.id,
                status: super::index_jobs::parse_index_job_status(&row.status),
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
                requested_by: user_summary_from_row(user),
            })
        })
        .collect();

    if let Some(query) = q {
        let query_lower = query.to_lowercase();
        dtos.retain(|d| {
            d.git_url.to_lowercase().contains(&query_lower)
                || format!("{:?}", d.status)
                    .to_lowercase()
                    .contains(&query_lower)
                || d.requested_by.handle.to_lowercase().contains(&query_lower)
                || d.repo_slug
                    .as_deref()
                    .map(|s| s.to_lowercase().contains(&query_lower))
                    .unwrap_or(false)
        });
    }

    let total = dtos.len() as i64;
    let start = offset.min(total) as usize;
    let mut page: Vec<AdminIndexJobDto> = dtos
        .into_iter()
        .skip(start)
        .take((limit + 1) as usize)
        .collect();

    let next_cursor = if page.len() as i64 > limit {
        page.truncate(limit as usize);
        Some((offset + limit).to_string())
    } else {
        None
    };

    Ok(AdminIndexJobListResponse {
        jobs: page,
        next_cursor,
    })
}

pub async fn cancel_index_job(
    auth: &AuthContext,
    job_id: Uuid,
) -> Result<CancelIndexJobResponse, AppError> {
    require_staff(auth)?;
    let mut conn = db_conn().await?;

    let row = index_jobs::table
        .find(job_id)
        .select(IndexJobRow::as_select())
        .first::<IndexJobRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound("index job not found".to_string()))?;

    let status = super::index_jobs::parse_index_job_status(&row.status);
    if !matches!(status, IndexJobStatus::Pending | IndexJobStatus::Running) {
        return Err(AppError::Conflict(format!(
            "cannot cancel job with status {:?}",
            status
        )));
    }

    let error_msg = format!("Cancelled by @{}", auth.user.handle);
    diesel::update(index_jobs::table.find(job_id))
        .set(IndexJobChangeset {
            status: Some("failed".to_string()),
            error_message: Some(Some(error_msg)),
            completed_at: Some(Some(chrono::Utc::now())),
            updated_at: Some(chrono::Utc::now()),
            ..Default::default()
        })
        .execute(&mut conn)
        .await?;

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "index_job.cancel",
        "index_job",
        Some(job_id),
        json!({ "git_url": row.git_url, "previous_status": row.status }),
    )
    .await?;

    Ok(CancelIndexJobResponse {
        ok: true,
        job_id,
        status: IndexJobStatus::Failed,
    })
}

pub async fn delete_repo(
    auth: &AuthContext,
    repo_id: Uuid,
) -> Result<AdminActionResponse, AppError> {
    require_admin(auth)?;
    let mut conn = db_conn().await?;

    let repo = repos::table
        .find(repo_id)
        .select(crate::models::RepoRow::as_select())
        .first::<crate::models::RepoRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound("repo not found".to_string()))?;

    diesel::delete(repos::table.find(repo_id))
        .execute(&mut conn)
        .await?;

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "repo.delete",
        "repo",
        Some(repo_id),
        json!({ "name": repo.name, "git_url": repo.git_url }),
    )
    .await?;

    Ok(AdminActionResponse {
        ok: true,
        message: format!("Repo '{}' deleted", repo.name),
    })
}

fn user_role_to_str(role: UserRole) -> &'static str {
    match role {
        UserRole::Admin => "admin",
        UserRole::Moderator => "moderator",
        UserRole::User => "user",
    }
}
