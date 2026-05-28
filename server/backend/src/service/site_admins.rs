use chrono::Utc;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::json;
use shared::{AddSiteAdminRequest, AdminActionResponse, SiteAdminDto, SiteAdminListResponse};
use uuid::Uuid;

use super::helpers::{db_conn, insert_audit_log, user_summary_from_row};
use crate::auth::{AuthContext, require_staff};
use crate::error::AppError;
use crate::models::{NewSiteAdminRow, SiteAdminRow, UserRow};
use crate::schema::{site_admins, users};

pub async fn list_site_admins(auth: &AuthContext) -> Result<SiteAdminListResponse, AppError> {
    require_staff(auth)?;
    let mut conn = db_conn().await?;

    let rows = site_admins::table
        .order(site_admins::created_at.desc())
        .select(SiteAdminRow::as_select())
        .load::<SiteAdminRow>(&mut conn)
        .await?;

    let user_ids: Vec<Uuid> = rows.iter().map(|r| r.user_id).collect();
    let granted_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.granted_by_user_id).collect();
    let all_ids: Vec<Uuid> = user_ids.iter().chain(granted_ids.iter()).copied().collect();

    let user_rows = users::table
        .filter(users::id.eq_any(&all_ids))
        .select(UserRow::as_select())
        .load::<UserRow>(&mut conn)
        .await?;

    let user_map: std::collections::HashMap<Uuid, &UserRow> =
        user_rows.iter().map(|u| (u.id, u)).collect();

    let admins = rows
        .iter()
        .filter_map(|row| {
            let user = user_map.get(&row.user_id)?;
            Some(SiteAdminDto {
                id: row.id,
                user: user_summary_from_row(user),
                granted_by: row
                    .granted_by_user_id
                    .and_then(|id| user_map.get(&id))
                    .map(|u| user_summary_from_row(u)),
                created_at: row.created_at,
            })
        })
        .collect();

    Ok(SiteAdminListResponse { admins })
}

pub async fn add_site_admin(
    auth: &AuthContext,
    request: AddSiteAdminRequest,
) -> Result<AdminActionResponse, AppError> {
    require_staff(auth)?;
    let mut conn = db_conn().await?;

    let target_user = users::table
        .filter(users::handle.eq(&request.user_handle))
        .select(UserRow::as_select())
        .first::<UserRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("user `{}` not found", request.user_handle)))?;

    // Check if already an admin
    let existing = site_admins::table
        .filter(site_admins::user_id.eq(target_user.id))
        .select(SiteAdminRow::as_select())
        .first::<SiteAdminRow>(&mut conn)
        .await
        .optional()?;

    if existing.is_some() {
        return Ok(AdminActionResponse {
            ok: true,
            message: format!("{} is already a site admin", request.user_handle),
        });
    }

    diesel::insert_into(site_admins::table)
        .values(NewSiteAdminRow {
            id: Uuid::now_v7(),
            user_id: target_user.id,
            granted_by_user_id: Some(auth.user.id),
            created_at: Utc::now(),
        })
        .execute(&mut conn)
        .await?;

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "site_admin.add",
        "user",
        Some(target_user.id),
        json!({ "handle": request.user_handle }),
    )
    .await?;

    Ok(AdminActionResponse {
        ok: true,
        message: format!("{} added as site admin", request.user_handle),
    })
}

pub async fn remove_site_admin(
    auth: &AuthContext,
    user_id: Uuid,
) -> Result<AdminActionResponse, AppError> {
    require_staff(auth)?;
    let mut conn = db_conn().await?;

    let deleted = diesel::delete(site_admins::table.filter(site_admins::user_id.eq(user_id)))
        .execute(&mut conn)
        .await?;

    if deleted == 0 {
        return Err(AppError::NotFound("site admin not found".to_string()));
    }

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "site_admin.remove",
        "user",
        Some(user_id),
        json!({}),
    )
    .await?;

    Ok(AdminActionResponse {
        ok: true,
        message: "Site admin removed".to_string(),
    })
}
