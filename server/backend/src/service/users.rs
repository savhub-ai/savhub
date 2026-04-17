use std::collections::{HashMap, HashSet};

use diesel::dsl::count_star;
use diesel::prelude::*;
use shared::{
    BrowseHistoryItem, FlockSummary, PagedResponse, SkillListItem, UserListItem, UserListResponse,
    UserProfileResponse, UserProfileStats, UserRole,
};
use uuid::Uuid;

use super::helpers::{
    db_conn, load_skill_versions_map, load_users_map, skill_item_from_rows, user_summary_from_row,
};
use super::repos::{flock_summary_from_row, load_flock_skill_counts};
use crate::auth::RequestUser;
use crate::error::AppError;
use crate::models::{BrowseHistoryRow, FlockRow, RepoRow, SkillRow, UserRow};
use crate::schema::{browse_histories, flocks, repos, skill_stars, skill_versions, skills, users};

const PROFILE_PAGE_SIZE_MAX: i64 = 100;
const PROFILE_HISTORY_PAGE_SIZE_MAX: i64 = 200;

pub fn list_users(query: Option<&str>, limit: i64) -> Result<UserListResponse, AppError> {
    let mut conn = db_conn()?;
    let limit = limit.clamp(1, 100) as usize;
    let users_rows = users::table
        .order(users::handle.asc())
        .select(UserRow::as_select())
        .load::<UserRow>(&mut conn)?;
    let filtered = users_rows
        .into_iter()
        .filter(|row| match query {
            Some(query) if !query.trim().is_empty() => {
                let query = query.trim().to_lowercase();
                row.handle.to_lowercase().contains(&query)
                    || row
                        .display_name
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
            }
            _ => true,
        })
        .collect::<Vec<_>>();

    let flock_rows = flocks::table
        .filter(flocks::soft_deleted_at.is_null())
        .select(FlockRow::as_select())
        .load::<FlockRow>(&mut conn)?;
    let mut flocks_by_importer: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for flock in &flock_rows {
        flocks_by_importer
            .entry(flock.imported_by_user_id)
            .or_default()
            .push(flock.id);
    }
    let skill_rows = skills::table
        .filter(skills::soft_deleted_at.is_null())
        .select(SkillRow::as_select())
        .load::<SkillRow>(&mut conn)?;
    let mut skill_counts_by_flock: HashMap<Uuid, i64> = HashMap::new();
    for row in &skill_rows {
        *skill_counts_by_flock.entry(row.flock_id).or_default() += 1;
    }
    let mut skill_counts: HashMap<Uuid, i64> = HashMap::new();
    for (importer_id, flock_ids) in &flocks_by_importer {
        let count: i64 = flock_ids
            .iter()
            .map(|fid| skill_counts_by_flock.get(fid).copied().unwrap_or(0))
            .sum();
        if count > 0 {
            skill_counts.insert(*importer_id, count);
        }
    }

    let total = filtered.len() as i64;
    let items = filtered
        .into_iter()
        .take(limit)
        .map(|row| UserListItem {
            user: user_summary_from_row(&row),
            skill_count: *skill_counts.get(&row.id).unwrap_or(&0),
        })
        .collect();

    Ok(UserListResponse { items, total })
}

pub fn get_user_profile(
    handle_value: &str,
    viewer: Option<&RequestUser>,
) -> Result<UserProfileResponse, AppError> {
    let mut conn = db_conn()?;
    let context = load_profile_context(&mut conn, handle_value, viewer)?;
    let published_skill_ids = published_skill_ids_for_user(&mut conn, context.user.id)?;
    let starred_skill_ids = if context.is_self {
        starred_skill_ids_for_user(&mut conn, context.user.id)?
    } else {
        Vec::new()
    };
    let starred_flock_ids = if context.is_self {
        starred_flock_ids_for_user(&mut conn, context.user.id)?
    } else {
        Vec::new()
    };
    let history_count = if context.is_self {
        browse_histories::table
            .filter(browse_histories::user_id.eq(context.user.id))
            .select(count_star())
            .first::<i64>(&mut conn)?
    } else {
        0
    };

    Ok(UserProfileResponse {
        user: user_summary_from_row(&context.user),
        bio: context.user.bio.clone(),
        joined_at: context.user.created_at,
        github_login: context.user.github_login.clone(),
        is_self: context.is_self,
        stats: UserProfileStats {
            published_skills: published_skill_ids.len() as i64,
            starred_skills: starred_skill_ids.len() as i64,
            starred_flocks: starred_flock_ids.len() as i64,
            history: history_count,
        },
    })
}

pub fn get_user_published_skills(
    handle_value: &str,
    viewer: Option<&RequestUser>,
    limit: i64,
    cursor: Option<String>,
) -> Result<PagedResponse<SkillListItem>, AppError> {
    let mut conn = db_conn()?;
    let context = load_profile_context(&mut conn, handle_value, viewer)?;
    let viewer_is_staff = viewer
        .map(|viewer| matches!(viewer.role, UserRole::Admin | UserRole::Moderator))
        .unwrap_or(false);
    let skill_ids = published_skill_ids_for_user(&mut conn, context.user.id)?;
    let (page_ids, next_cursor) = paginate_unique_ids(skill_ids, limit, cursor, PROFILE_PAGE_SIZE_MAX);
    let items = load_skill_items(&mut conn, page_ids, viewer_is_staff)?;
    Ok(PagedResponse { items, next_cursor })
}

pub fn get_user_starred_skills(
    handle_value: &str,
    viewer: Option<&RequestUser>,
    limit: i64,
    cursor: Option<String>,
) -> Result<PagedResponse<SkillListItem>, AppError> {
    let mut conn = db_conn()?;
    let context = load_profile_context(&mut conn, handle_value, viewer)?;
    if !context.is_self {
        return Err(AppError::Forbidden(
            "starred skills are only visible on your own profile".to_string(),
        ));
    }
    let viewer_is_staff = viewer
        .map(|viewer| matches!(viewer.role, UserRole::Admin | UserRole::Moderator))
        .unwrap_or(false);
    let skill_ids = starred_skill_ids_for_user(&mut conn, context.user.id)?;
    let (page_ids, next_cursor) = paginate_unique_ids(skill_ids, limit, cursor, PROFILE_PAGE_SIZE_MAX);
    let items = load_skill_items(&mut conn, page_ids, viewer_is_staff)?;
    Ok(PagedResponse { items, next_cursor })
}

pub fn get_user_starred_flocks(
    handle_value: &str,
    viewer: Option<&RequestUser>,
    limit: i64,
    cursor: Option<String>,
) -> Result<PagedResponse<FlockSummary>, AppError> {
    let mut conn = db_conn()?;
    let context = load_profile_context(&mut conn, handle_value, viewer)?;
    if !context.is_self {
        return Err(AppError::Forbidden(
            "starred flocks are only visible on your own profile".to_string(),
        ));
    }
    let flock_ids = starred_flock_ids_for_user(&mut conn, context.user.id)?;
    let (page_ids, next_cursor) = paginate_unique_ids(flock_ids, limit, cursor, PROFILE_PAGE_SIZE_MAX);
    let items = load_flock_items(&mut conn, page_ids)?;
    Ok(PagedResponse { items, next_cursor })
}

pub fn get_user_history(
    handle_value: &str,
    viewer: Option<&RequestUser>,
    limit: i64,
    cursor: Option<String>,
) -> Result<PagedResponse<BrowseHistoryItem>, AppError> {
    let mut conn = db_conn()?;
    let context = load_profile_context(&mut conn, handle_value, viewer)?;
    if !context.is_self {
        return Err(AppError::Forbidden(
            "browse history is only visible on your own profile".to_string(),
        ));
    }
    let limit = limit.clamp(1, PROFILE_HISTORY_PAGE_SIZE_MAX);
    let offset = parse_cursor(cursor);
    let mut rows = browse_histories::table
        .filter(browse_histories::user_id.eq(context.user.id))
        .order(browse_histories::viewed_at.desc())
        .limit(limit + 1)
        .offset(offset)
        .select(BrowseHistoryRow::as_select())
        .load::<BrowseHistoryRow>(&mut conn)?;

    let next_cursor = if rows.len() > limit as usize {
        rows.pop();
        Some((offset + limit).to_string())
    } else {
        None
    };

    let items = super::browse_history::hydrate_history_items_for_user_page(&mut conn, rows)?;
    Ok(PagedResponse { items, next_cursor })
}

struct ProfileContext {
    user: UserRow,
    is_self: bool,
}

fn load_profile_context(
    conn: &mut PgConnection,
    handle_value: &str,
    viewer: Option<&RequestUser>,
) -> Result<ProfileContext, AppError> {
    let user = users::table
        .filter(users::handle.eq(handle_value))
        .select(UserRow::as_select())
        .first::<UserRow>(conn)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("user `{handle_value}` not found")))?;
    let is_self = viewer.map(|viewer| viewer.id == user.id).unwrap_or(false);
    Ok(ProfileContext { user, is_self })
}

fn published_skill_ids_for_user(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    let ids = skill_versions::table
        .filter(skill_versions::created_by.eq(user_id))
        .filter(skill_versions::soft_deleted_at.is_null())
        .order(skill_versions::created_at.desc())
        .select(skill_versions::skill_id)
        .load::<Option<Uuid>>(conn)?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    Ok(ordered_unique_ids(ids))
}

fn starred_skill_ids_for_user(conn: &mut PgConnection, user_id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let ids = skill_stars::table
        .filter(skill_stars::user_id.eq(user_id))
        .filter(skill_stars::skill_id.is_not_null())
        .order(skill_stars::created_at.desc())
        .select(skill_stars::skill_id)
        .load::<Option<Uuid>>(conn)?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    Ok(ordered_unique_ids(ids))
}

fn starred_flock_ids_for_user(conn: &mut PgConnection, user_id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let ids = skill_stars::table
        .filter(skill_stars::user_id.eq(user_id))
        .filter(skill_stars::skill_id.is_null())
        .order(skill_stars::created_at.desc())
        .select(skill_stars::flock_id)
        .load::<Uuid>(conn)?;
    Ok(ordered_unique_ids(ids))
}

fn paginate_unique_ids(
    ids: Vec<Uuid>,
    limit: i64,
    cursor: Option<String>,
    max_limit: i64,
) -> (Vec<Uuid>, Option<String>) {
    let limit = limit.clamp(1, max_limit) as usize;
    let offset = parse_cursor(cursor) as usize;
    if offset >= ids.len() {
        return (Vec::new(), None);
    }
    let end = (offset + limit).min(ids.len());
    let next_cursor = if end < ids.len() {
        Some(end.to_string())
    } else {
        None
    };
    (ids[offset..end].to_vec(), next_cursor)
}

fn parse_cursor(cursor: Option<String>) -> i64 {
    cursor
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0)
}

fn load_flock_items(
    conn: &mut PgConnection,
    ordered_flock_ids: Vec<Uuid>,
) -> Result<Vec<FlockSummary>, AppError> {
    if ordered_flock_ids.is_empty() {
        return Ok(Vec::new());
    }

    let flock_rows = flocks::table
        .filter(flocks::id.eq_any(&ordered_flock_ids))
        .filter(flocks::soft_deleted_at.is_null())
        .select(FlockRow::as_select())
        .load::<FlockRow>(conn)?;
    let repo_rows = repos::table
        .filter(repos::id.eq_any(flock_rows.iter().map(|row| row.repo_id).collect::<Vec<_>>()))
        .select(RepoRow::as_select())
        .load::<RepoRow>(conn)?;
    let importing_users = load_users_map(
        conn,
        flock_rows
            .iter()
            .map(|row| row.imported_by_user_id)
            .collect::<Vec<_>>(),
    )?;
    let skill_counts = load_flock_skill_counts(conn, ordered_flock_ids.clone())?;

    let flock_map = flock_rows
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();
    let repo_map = repo_rows
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();

    ordered_flock_ids
        .into_iter()
        .filter_map(|flock_id| {
            flock_map
                .get(&flock_id)
                .and_then(|flock| repo_map.get(&flock.repo_id).map(|repo| (flock, repo)))
        })
        .map(|(flock, repo)| {
            let skill_count = *skill_counts.get(&flock.id).unwrap_or(&0);
            flock_summary_from_row(flock, repo, &importing_users, skill_count)
        })
        .collect()
}

fn load_skill_items(
    conn: &mut PgConnection,
    ordered_skill_ids: Vec<Uuid>,
    viewer_is_staff: bool,
) -> Result<Vec<SkillListItem>, AppError> {
    if ordered_skill_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut query = skills::table
        .filter(skills::id.eq_any(&ordered_skill_ids))
        .filter(skills::soft_deleted_at.is_null())
        .into_boxed();
    if !viewer_is_staff {
        query = query.filter(skills::moderation_status.eq("active"));
    }
    let skill_rows = query.select(SkillRow::as_select()).load::<SkillRow>(conn)?;
    let flock_ids: Vec<Uuid> = skill_rows
        .iter()
        .map(|row| row.flock_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let flock_rows = flocks::table
        .filter(flocks::id.eq_any(&flock_ids))
        .select(FlockRow::as_select())
        .load::<FlockRow>(conn)?;
    let owner_ids: Vec<Uuid> = flock_rows.iter().map(|f| f.imported_by_user_id).collect();
    let owners = load_users_map(conn, owner_ids)?;
    let flock_owner_map: HashMap<Uuid, Uuid> = flock_rows
        .iter()
        .map(|f| (f.id, f.imported_by_user_id))
        .collect();

    let latest_versions = load_skill_versions_map(
        conn,
        skill_rows
            .iter()
            .filter_map(|row| row.latest_version_id)
            .collect::<Vec<_>>(),
    )?;
    let repo_ids: Vec<Uuid> = skill_rows
        .iter()
        .map(|row| row.repo_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let repo_rows = repos::table
        .filter(repos::id.eq_any(&repo_ids))
        .select(RepoRow::as_select())
        .load::<RepoRow>(conn)?;
    let repo_map: HashMap<Uuid, RepoRow> = repo_rows.into_iter().map(|r| (r.id, r)).collect();

    let skill_map = skill_rows
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();

    ordered_skill_ids
        .into_iter()
        .filter_map(|skill_id| skill_map.get(&skill_id))
        .map(|row| {
            let owner_user_id = flock_owner_map
                .get(&row.flock_id)
                .copied()
                .unwrap_or(Uuid::nil());
            let owner = owners
                .get(&owner_user_id)
                .ok_or_else(|| AppError::Internal("missing skill owner".to_string()))?;
            let latest = row.latest_version_id.and_then(|id| latest_versions.get(&id));
            let git_url = repo_map
                .get(&row.repo_id)
                .map(|r| r.git_url.as_str())
                .unwrap_or_default();
            Ok(skill_item_from_rows(row, git_url, owner, latest))
        })
        .collect()
}

fn ordered_unique_ids<I>(ids: I) -> Vec<Uuid>
where
    I: IntoIterator<Item = Uuid>,
{
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for id in ids {
        if seen.insert(id) {
            ordered.push(id);
        }
    }
    ordered
}
