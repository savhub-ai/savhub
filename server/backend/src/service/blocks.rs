use chrono::Utc;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::json;
use shared::{BlockedFlockDto, DeleteResponse, FlockBlockListResponse};
use uuid::Uuid;

use super::helpers::{db_conn, fetch_flock_by_path, insert_audit_log};
use crate::auth::AuthContext;
use crate::error::AppError;
use crate::models::{FlockRow, NewSkillBlockRow, RepoRow, SkillBlockRow};
use crate::schema::{flocks, repos, skill_blocks};

pub async fn block_flock(
    auth: &AuthContext,
    repo_domain: &str,
    repo_path_slug: &str,
    flock_slug: &str,
) -> Result<DeleteResponse, AppError> {
    let mut conn = db_conn().await?;
    let repo_sign = format!("{repo_domain}/{repo_path_slug}");
    let flock = fetch_flock_by_path(&mut conn, &repo_sign, flock_slug).await?;

    let existing = skill_blocks::table
        .filter(skill_blocks::user_id.eq(auth.user.id))
        .filter(skill_blocks::flock_id.eq(flock.id))
        .filter(skill_blocks::skill_id.is_null())
        .select(SkillBlockRow::as_select())
        .first::<SkillBlockRow>(&mut conn)
        .await
        .optional()?;
    if existing.is_some() {
        return Err(AppError::Conflict("flock is already blocked".to_string()));
    }

    diesel::insert_into(skill_blocks::table)
        .values(NewSkillBlockRow {
            id: Uuid::now_v7(),
            user_id: auth.user.id,
            repo_id: flock.repo_id,
            flock_id: flock.id,
            skill_id: None,
            created_at: Utc::now(),
        })
        .execute(&mut conn)
        .await?;

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "flock.block",
        "flock",
        Some(flock.id),
        json!({ "repo": format!("{}/{}", repo_domain, repo_path_slug), "flock_slug": flock_slug }),
    )
    .await?;

    Ok(DeleteResponse { ok: true })
}

pub async fn unblock_flock(
    auth: &AuthContext,
    repo_domain: &str,
    repo_path_slug: &str,
    flock_slug: &str,
) -> Result<DeleteResponse, AppError> {
    let mut conn = db_conn().await?;
    let repo_sign = format!("{repo_domain}/{repo_path_slug}");
    let flock = fetch_flock_by_path(&mut conn, &repo_sign, flock_slug).await?;

    let deleted = diesel::delete(
        skill_blocks::table
            .filter(skill_blocks::user_id.eq(auth.user.id))
            .filter(skill_blocks::flock_id.eq(flock.id))
            .filter(skill_blocks::skill_id.is_null()),
    )
    .execute(&mut conn)
    .await?;

    if deleted == 0 {
        return Err(AppError::NotFound("flock is not blocked".to_string()));
    }

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "flock.unblock",
        "flock",
        Some(flock.id),
        json!({ "repo": format!("{}/{}", repo_domain, repo_path_slug), "flock_slug": flock_slug }),
    )
    .await?;

    Ok(DeleteResponse { ok: true })
}

pub async fn list_blocked_flocks(auth: &AuthContext) -> Result<FlockBlockListResponse, AppError> {
    let mut conn = db_conn().await?;
    let rows = skill_blocks::table
        .filter(skill_blocks::user_id.eq(auth.user.id))
        .filter(skill_blocks::skill_id.is_null())
        .order(skill_blocks::created_at.desc())
        .select(SkillBlockRow::as_select())
        .load::<SkillBlockRow>(&mut conn)
        .await?;

    let flock_ids: Vec<Uuid> = rows.iter().map(|r| r.flock_id).collect();
    if flock_ids.is_empty() {
        return Ok(FlockBlockListResponse {
            blocked_flocks: vec![],
        });
    }

    let flock_rows = flocks::table
        .filter(flocks::id.eq_any(&flock_ids))
        .select(FlockRow::as_select())
        .load::<FlockRow>(&mut conn)
        .await?;
    let repo_ids: Vec<Uuid> = flock_rows.iter().map(|f| f.repo_id).collect();
    let repo_rows = repos::table
        .filter(repos::id.eq_any(&repo_ids))
        .select(RepoRow::as_select())
        .load::<RepoRow>(&mut conn)
        .await?;

    let blocked_flocks = rows
        .into_iter()
        .filter_map(|block| {
            let flock = flock_rows.iter().find(|f| f.id == block.flock_id)?;
            let repo = repo_rows.iter().find(|r| r.id == flock.repo_id)?;
            Some(BlockedFlockDto {
                flock_id: flock.id,
                repo_slug: super::helpers::derive_repo_sign(&repo.git_url),
                flock_slug: flock.slug.clone(),
                flock_name: flock.name.clone(),
                blocked_at: block.created_at,
            })
        })
        .collect();

    Ok(FlockBlockListResponse { blocked_flocks })
}
