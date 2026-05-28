use chrono::Utc;
use diesel::dsl::count_star;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::NewUserRow;
use crate::schema::{user_tokens, users};

pub async fn ensure_seed_data(pool: &crate::db::AsyncPgPool) -> Result<(), AppError> {
    let mut conn = pool
        .get()
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    prune_legacy_demo_tokens(&mut conn).await?;

    let user_count = users::table
        .select(count_star())
        .first::<i64>(&mut conn)
        .await?;
    if user_count == 0 {
        insert_demo_users(&mut conn).await?;
    }

    Ok(())
}

async fn prune_legacy_demo_tokens(conn: &mut AsyncPgConnection) -> Result<(), AppError> {
    diesel::delete(user_tokens::table.filter(user_tokens::token.eq_any(vec![
        "savhub_admin_demo",
        "savhub_creator_demo",
        "savhub_viewer_demo",
    ])))
    .execute(conn)
    .await?;
    Ok(())
}

async fn insert_demo_users(conn: &mut AsyncPgConnection) -> Result<(), AppError> {
    let now = Utc::now();
    let admin_id = Uuid::now_v7();
    let creator_id = Uuid::now_v7();
    let viewer_id = Uuid::now_v7();

    diesel::insert_into(users::table)
        .values(vec![
            NewUserRow {
                id: admin_id,
                handle: "admin".to_string(),
                display_name: Some("Savhub Admin".to_string()),
                bio: Some("Seeded platform admin account.".to_string()),
                avatar_url: None,
                github_user_id: None,
                github_login: None,
                role: "admin".to_string(),
                created_at: now,
                updated_at: now,
            },
            NewUserRow {
                id: creator_id,
                handle: "savfox".to_string(),
                display_name: Some("Savfox Team".to_string()),
                bio: Some("Default seeded publisher account.".to_string()),
                avatar_url: None,
                github_user_id: None,
                github_login: None,
                role: "moderator".to_string(),
                created_at: now,
                updated_at: now,
            },
            NewUserRow {
                id: viewer_id,
                handle: "reader".to_string(),
                display_name: Some("Registry Reader".to_string()),
                bio: Some("Default seeded viewer account.".to_string()),
                avatar_url: None,
                github_user_id: None,
                github_login: None,
                role: "user".to_string(),
                created_at: now,
                updated_at: now,
            },
        ])
        .execute(conn)
        .await?;

    Ok(())
}
