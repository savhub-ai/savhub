use std::collections::HashMap;

use chrono::Utc;
use diesel::prelude::*;
use shared::{BrowseHistoryItem, BrowseHistoryResponse, RecordViewRequest};
use uuid::Uuid;

use super::helpers::{db_conn, load_users_map};
use crate::auth::AuthContext;
use crate::error::AppError;
use crate::models::{BrowseHistoryRow, FlockRow, NewBrowseHistoryRow, SkillRow};
use crate::schema::{browse_histories, flocks, skills};

/// Record a page view. Upserts: if the same user+resource already exists,
/// update the viewed_at timestamp instead of inserting a duplicate.
pub fn record_view(auth: &AuthContext, request: RecordViewRequest) -> Result<(), AppError> {
    let mut conn = db_conn()?;
    let now = Utc::now();

    // Check for existing entry for this user + resource
    let existing = browse_histories::table
        .filter(browse_histories::user_id.eq(auth.user.id))
        .filter(browse_histories::resource_type.eq(&request.resource_type))
        .filter(browse_histories::resource_id.eq(request.resource_id))
        .select(BrowseHistoryRow::as_select())
        .first::<BrowseHistoryRow>(&mut conn)
        .optional()?;

    if let Some(row) = existing {
        diesel::update(browse_histories::table.find(row.id))
            .set(browse_histories::viewed_at.eq(now))
            .execute(&mut conn)?;
    } else {
        diesel::insert_into(browse_histories::table)
            .values(NewBrowseHistoryRow {
                id: Uuid::now_v7(),
                user_id: auth.user.id,
                resource_type: request.resource_type,
                resource_id: request.resource_id,
                viewed_at: now,
            })
            .execute(&mut conn)?;
    }

    Ok(())
}

/// Get a user's browse history, most recent first, limited to `limit` items.
pub fn get_history(auth: &AuthContext, limit: i64) -> Result<BrowseHistoryResponse, AppError> {
    let mut conn = db_conn()?;
    get_history_for_user_id_with_conn(&mut conn, auth.user.id, limit)
}

pub fn get_history_for_user_id(
    user_id: Uuid,
    limit: i64,
) -> Result<BrowseHistoryResponse, AppError> {
    let mut conn = db_conn()?;
    get_history_for_user_id_with_conn(&mut conn, user_id, limit)
}

pub fn get_history_for_user_id_with_conn(
    conn: &mut PgConnection,
    user_id: Uuid,
    limit: i64,
) -> Result<BrowseHistoryResponse, AppError> {
    let rows = browse_histories::table
        .filter(browse_histories::user_id.eq(user_id))
        .order(browse_histories::viewed_at.desc())
        .limit(limit.clamp(1, 200))
        .select(BrowseHistoryRow::as_select())
        .load::<BrowseHistoryRow>(conn)?;

    Ok(BrowseHistoryResponse {
        items: hydrate_history_items_for_user_page(conn, rows)?,
    })
}

/// Delete browse_histories entries older than 365 days.
pub fn cleanup_old_history(conn: &mut PgConnection) -> Result<usize, AppError> {
    let cutoff = Utc::now() - chrono::Duration::days(365);
    let deleted =
        diesel::delete(browse_histories::table.filter(browse_histories::viewed_at.lt(cutoff)))
            .execute(conn)?;
    Ok(deleted)
}

pub(crate) fn hydrate_history_items_for_user_page(
    conn: &mut PgConnection,
    rows: Vec<BrowseHistoryRow>,
) -> Result<Vec<BrowseHistoryItem>, AppError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let skill_ids = rows
        .iter()
        .filter(|row| row.resource_type == "skill")
        .map(|row| row.resource_id)
        .collect::<Vec<_>>();
    let flock_ids = rows
        .iter()
        .filter(|row| row.resource_type == "flock")
        .map(|row| row.resource_id)
        .collect::<Vec<_>>();

    let skill_rows = if skill_ids.is_empty() {
        Vec::new()
    } else {
        skills::table
            .filter(skills::id.eq_any(&skill_ids))
            .select(SkillRow::as_select())
            .load::<SkillRow>(conn)?
    };
    let flock_rows = if flock_ids.is_empty() {
        Vec::new()
    } else {
        flocks::table
            .filter(flocks::id.eq_any(&flock_ids))
            .select(FlockRow::as_select())
            .load::<FlockRow>(conn)?
    };

    let user_ids = flock_rows
        .iter()
        .map(|row| row.imported_by_user_id)
        .collect::<Vec<_>>();
    let users = load_users_map(conn, user_ids)?;

    let skill_map = skill_rows
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();
    let flock_map = flock_rows
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();

    Ok(rows
        .into_iter()
        .map(|row| {
            let (resource_slug, resource_title, owner_handle) = match row.resource_type.as_str() {
                "skill" => skill_map
                    .get(&row.resource_id)
                    .map(|skill| (skill.slug.clone(), skill.name.clone(), None))
                    .unwrap_or_else(|| (String::new(), String::new(), None)),
                "flock" => flock_map
                    .get(&row.resource_id)
                    .map(|flock| {
                        (
                            flock.slug.clone(),
                            flock.name.clone(),
                            users
                                .get(&flock.imported_by_user_id)
                                .map(|user| user.handle.clone()),
                        )
                    })
                    .unwrap_or_else(|| (String::new(), String::new(), None)),
                _ => (String::new(), String::new(), None),
            };

            BrowseHistoryItem {
                resource_type: row.resource_type,
                resource_id: row.resource_id,
                resource_slug,
                resource_title,
                owner_handle,
                viewed_at: row.viewed_at,
            }
        })
        .collect())
}
