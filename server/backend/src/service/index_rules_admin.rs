use chrono::Utc;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use shared::{
    AdminActionResponse, CreateIndexRuleRequest, IndexRuleDto, IndexRuleListResponse,
    UpdateIndexRuleRequest,
};
use uuid::Uuid;

use super::helpers::{db_conn, normalize_git_url};
use crate::auth::{AuthContext, require_admin};
use crate::error::AppError;
use crate::models::{IndexRuleChangeset, IndexRuleRow, NewIndexRuleRow};
use crate::schema::index_rules;

pub async fn list_index_rules(
    auth: &AuthContext,
    q: Option<&str>,
    limit: i64,
    cursor: Option<String>,
) -> Result<IndexRuleListResponse, AppError> {
    require_admin(auth)?;
    let mut conn = db_conn().await?;

    let offset: i64 = cursor
        .as_deref()
        .and_then(|c| c.parse::<i64>().ok())
        .unwrap_or(0);
    let limit = limit.clamp(1, 100);

    let rows = index_rules::table
        .order(index_rules::repo_url.asc())
        .select(IndexRuleRow::as_select())
        .load::<IndexRuleRow>(&mut conn)
        .await?;

    let mut dtos: Vec<IndexRuleDto> = rows.iter().map(dto_from_row).collect();

    if let Some(query) = q {
        let query_lower = query.to_lowercase();
        dtos.retain(|r| {
            r.repo_url.to_lowercase().contains(&query_lower)
                || r.path_regex.to_lowercase().contains(&query_lower)
                || r.description.to_lowercase().contains(&query_lower)
        });
    }

    let total = dtos.len() as i64;
    let start = offset.min(total) as usize;
    let mut page: Vec<IndexRuleDto> = dtos
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

    Ok(IndexRuleListResponse {
        rules: page,
        next_cursor,
    })
}

pub async fn create_index_rule(
    auth: &AuthContext,
    request: CreateIndexRuleRequest,
) -> Result<IndexRuleDto, AppError> {
    require_admin(auth)?;

    if request.repo_url.trim().is_empty() {
        return Err(AppError::BadRequest("repo_url is required".to_string()));
    }
    if request.strategy.trim().is_empty() {
        return Err(AppError::BadRequest("strategy is required".to_string()));
    }

    let repo_url = normalize_git_url(&request.repo_url);

    let mut conn = db_conn().await?;
    let now = Utc::now();

    let existing = index_rules::table
        .filter(index_rules::repo_url.eq(&repo_url))
        .filter(index_rules::path_regex.eq(&request.path_regex))
        .select(IndexRuleRow::as_select())
        .first::<IndexRuleRow>(&mut conn)
        .await
        .optional()?;

    if existing.is_some() {
        return Err(AppError::Conflict(format!(
            "index rule for `{}` path `{}` already exists",
            repo_url, request.path_regex
        )));
    }

    let row = NewIndexRuleRow {
        id: Uuid::now_v7(),
        repo_url,
        path_regex: request.path_regex,
        strategy: request.strategy,
        description: request.description,
        created_at: now,
        updated_at: now,
    };

    diesel::insert_into(index_rules::table)
        .values(&row)
        .execute(&mut conn)
        .await?;

    let inserted = index_rules::table
        .find(row.id)
        .select(IndexRuleRow::as_select())
        .first::<IndexRuleRow>(&mut conn)
        .await?;

    Ok(dto_from_row(&inserted))
}

pub async fn update_index_rule(
    auth: &AuthContext,
    id: Uuid,
    request: UpdateIndexRuleRequest,
) -> Result<IndexRuleDto, AppError> {
    require_admin(auth)?;
    let mut conn = db_conn().await?;

    let _existing = index_rules::table
        .find(id)
        .select(IndexRuleRow::as_select())
        .first::<IndexRuleRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound("index rule not found".to_string()))?;

    let changeset = IndexRuleChangeset {
        repo_url: request.repo_url.map(|u| normalize_git_url(&u)),
        path_regex: request.path_regex,
        strategy: request.strategy,
        description: request.description,
        updated_at: Some(Utc::now()),
    };

    diesel::update(index_rules::table.find(id))
        .set(&changeset)
        .execute(&mut conn)
        .await?;

    let updated = index_rules::table
        .find(id)
        .select(IndexRuleRow::as_select())
        .first::<IndexRuleRow>(&mut conn)
        .await?;

    Ok(dto_from_row(&updated))
}

pub async fn delete_index_rule(
    auth: &AuthContext,
    id: Uuid,
) -> Result<AdminActionResponse, AppError> {
    require_admin(auth)?;
    let mut conn = db_conn().await?;

    let deleted = diesel::delete(index_rules::table.find(id))
        .execute(&mut conn)
        .await?;

    if deleted == 0 {
        return Err(AppError::NotFound("index rule not found".to_string()));
    }

    Ok(AdminActionResponse {
        ok: true,
        message: "Index rule deleted".to_string(),
    })
}

fn dto_from_row(row: &IndexRuleRow) -> IndexRuleDto {
    IndexRuleDto {
        id: row.id,
        repo_url: row.repo_url.clone(),
        path_regex: row.path_regex.clone(),
        strategy: row.strategy.clone(),
        description: row.description.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}
