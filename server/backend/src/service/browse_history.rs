use std::collections::HashMap;

use chrono::Utc;
use diesel::prelude::*;
use shared::{BrowseHistoryItem, BrowseHistoryResponse, RecordViewRequest};
use uuid::Uuid;

use super::helpers::{db_conn, load_users_map};
use crate::auth::AuthContext;
use crate::error::AppError;
use crate::models::{BrowseHistoryRow, FlockRow, NewBrowseHistoryRow, RepoRow, SkillRow};
use crate::schema::{browse_histories, flocks, repos, skills};

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
    let repo_ids = rows
        .iter()
        .filter(|row| row.resource_type == "repo")
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
    let repo_rows = if repo_ids.is_empty() {
        Vec::new()
    } else {
        repos::table
            .filter(repos::id.eq_any(&repo_ids))
            .select(RepoRow::as_select())
            .load::<RepoRow>(conn)?
    };

    let user_ids = flock_rows
        .iter()
        .map(|row| row.imported_by_user_id)
        .collect::<Vec<_>>();
    let users = load_users_map(conn, user_ids)?;

    let skill_map = skill_rows
        .into_iter()
        .map(|row| (row.id, (row.slug, row.name)))
        .collect::<HashMap<_, _>>();
    let flock_map = flock_rows
        .into_iter()
        .map(|row| {
            let owner_handle = users
                .get(&row.imported_by_user_id)
                .map(|user| user.handle.clone());
            (row.id, (row.slug, row.name, owner_handle))
        })
        .collect::<HashMap<_, _>>();
    let repo_map = repo_rows
        .into_iter()
        .map(|row| {
            (
                row.id,
                (super::helpers::derive_repo_sign(&row.git_url), row.name),
            )
        })
        .collect::<HashMap<_, _>>();

    Ok(rows
        .into_iter()
        .map(|row| {
            let (resource_slug, resource_title, owner_handle) = history_item_fields(
                &row.resource_type,
                row.resource_id,
                &skill_map,
                &flock_map,
                &repo_map,
            );

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

fn history_item_fields(
    resource_type: &str,
    resource_id: Uuid,
    skill_map: &HashMap<Uuid, (String, String)>,
    flock_map: &HashMap<Uuid, (String, String, Option<String>)>,
    repo_map: &HashMap<Uuid, (String, String)>,
) -> (String, String, Option<String>) {
    match resource_type {
        "skill" => skill_map
            .get(&resource_id)
            .map(|(slug, title)| (slug.clone(), title.clone(), None))
            .unwrap_or_else(|| (String::new(), String::new(), None)),
        "flock" => flock_map
            .get(&resource_id)
            .map(|(slug, title, owner_handle)| (slug.clone(), title.clone(), owner_handle.clone()))
            .unwrap_or_else(|| (String::new(), String::new(), None)),
        "repo" => repo_map
            .get(&resource_id)
            .map(|(slug, title)| (slug.clone(), title.clone(), None))
            .unwrap_or_else(|| (String::new(), String::new(), None)),
        _ => (String::new(), String::new(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::history_item_fields;
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn history_item_fields_populates_repo_entries() {
        let resource_id = Uuid::now_v7();
        let skill_map = HashMap::new();
        let flock_map = HashMap::new();
        let repo_map = HashMap::from([(
            resource_id,
            (
                "github.com/chrislearn/plugins".to_string(),
                "plugins".to_string(),
            ),
        )]);

        let (slug, title, owner_handle) =
            history_item_fields("repo", resource_id, &skill_map, &flock_map, &repo_map);

        assert_eq!(slug, "github.com/chrislearn/plugins");
        assert_eq!(title, "plugins");
        assert_eq!(owner_handle, None);
    }
}
