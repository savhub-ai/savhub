use chrono::Utc;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde_json::json;
use shared::{
    CreateReportRequest, ReportDto, ReportListResponse, ReportReason, ReportStatus,
    ReportTargetType, ReviewReportRequest,
};
use uuid::Uuid;

use super::helpers::{db_conn, insert_audit_log, load_users_map, user_summary_from_row};
use crate::auth::AuthContext;
use crate::error::AppError;
use crate::models::{NewReportRow, ReportRow};
use crate::schema::reports;

pub async fn create_report(
    auth: &AuthContext,
    request: CreateReportRequest,
) -> Result<ReportDto, AppError> {
    let mut conn = db_conn().await?;
    let now = Utc::now();
    let id = Uuid::now_v7();

    let existing = reports::table
        .filter(reports::reporter_user_id.eq(auth.user.id))
        .filter(reports::target_type.eq(target_type_to_str(request.target_type)))
        .filter(reports::target_id.eq(request.target_id))
        .filter(reports::status.eq("pending"))
        .select(ReportRow::as_select())
        .first::<ReportRow>(&mut conn)
        .await
        .optional()?;
    if existing.is_some() {
        return Err(AppError::Conflict(
            "you already have a pending report for this target".to_string(),
        ));
    }

    diesel::insert_into(reports::table)
        .values(NewReportRow {
            id,
            reporter_user_id: auth.user.id,
            target_type: target_type_to_str(request.target_type).to_string(),
            target_id: request.target_id,
            reason: reason_to_str(request.reason).to_string(),
            description: request.description.clone(),
            status: "pending".to_string(),
            reviewed_by_user_id: None,
            reviewed_at: None,
            notes: None,
            created_at: now,
        })
        .execute(&mut conn)
        .await?;

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "report.create",
        "report",
        Some(id),
        json!({
            "target_type": target_type_to_str(request.target_type),
            "target_id": request.target_id,
            "reason": reason_to_str(request.reason),
        }),
    )
    .await?;

    let row = reports::table
        .find(id)
        .select(ReportRow::as_select())
        .first::<ReportRow>(&mut conn)
        .await?;
    report_dto_from_row(&mut conn, row).await
}

pub async fn list_reports(auth: &AuthContext) -> Result<ReportListResponse, AppError> {
    if !matches!(
        auth.user.role,
        shared::UserRole::Admin | shared::UserRole::Moderator
    ) {
        return Err(AppError::Forbidden(
            "moderator or admin access required".to_string(),
        ));
    }
    let mut conn = db_conn().await?;
    let rows = reports::table
        .order(reports::created_at.desc())
        .limit(100)
        .select(ReportRow::as_select())
        .load::<ReportRow>(&mut conn)
        .await?;

    let mut reports = Vec::with_capacity(rows.len());
    for row in rows {
        reports.push(report_dto_from_row(&mut conn, row).await?);
    }

    Ok(ReportListResponse { reports })
}

pub async fn review_report(
    auth: &AuthContext,
    report_id: Uuid,
    request: ReviewReportRequest,
) -> Result<ReportDto, AppError> {
    if !matches!(
        auth.user.role,
        shared::UserRole::Admin | shared::UserRole::Moderator
    ) {
        return Err(AppError::Forbidden(
            "moderator or admin access required".to_string(),
        ));
    }
    let mut conn = db_conn().await?;
    let report = reports::table
        .find(report_id)
        .select(ReportRow::as_select())
        .first::<ReportRow>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound("report not found".to_string()))?;
    if report.status != "pending" {
        return Err(AppError::Conflict(
            "report has already been reviewed".to_string(),
        ));
    }

    let now = Utc::now();
    diesel::update(reports::table.find(report_id))
        .set((
            reports::status.eq(report_status_to_str(request.status)),
            reports::reviewed_by_user_id.eq(Some(auth.user.id)),
            reports::reviewed_at.eq(Some(now)),
            reports::notes.eq(request.notes.as_deref()),
        ))
        .execute(&mut conn)
        .await?;

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "report.review",
        "report",
        Some(report_id),
        json!({
            "status": report_status_to_str(request.status),
            "notes": request.notes,
        }),
    )
    .await?;

    let row = reports::table
        .find(report_id)
        .select(ReportRow::as_select())
        .first::<ReportRow>(&mut conn)
        .await?;
    report_dto_from_row(&mut conn, row).await
}

async fn report_dto_from_row(
    conn: &mut AsyncPgConnection,
    row: ReportRow,
) -> Result<ReportDto, AppError> {
    let mut user_ids = vec![row.reporter_user_id];
    if let Some(reviewer_id) = row.reviewed_by_user_id {
        user_ids.push(reviewer_id);
    }
    let users = load_users_map(conn, user_ids).await?;
    let reporter = users
        .get(&row.reporter_user_id)
        .ok_or_else(|| AppError::Internal("missing reporter".to_string()))?;
    let reviewed_by = row
        .reviewed_by_user_id
        .and_then(|id| users.get(&id))
        .map(user_summary_from_row);

    Ok(ReportDto {
        id: row.id,
        reporter: user_summary_from_row(reporter),
        target_type: parse_target_type(&row.target_type),
        target_id: row.target_id,
        reason: parse_reason(&row.reason),
        description: row.description,
        status: parse_report_status(&row.status),
        reviewed_by,
        reviewed_at: row.reviewed_at,
        notes: row.notes,
        created_at: row.created_at,
    })
}

fn target_type_to_str(value: ReportTargetType) -> &'static str {
    match value {
        ReportTargetType::Skill => "skill",
        ReportTargetType::Flock => "flock",
        ReportTargetType::Comment => "comment",
        ReportTargetType::User => "user",
    }
}

fn parse_target_type(value: &str) -> ReportTargetType {
    match value {
        "flock" => ReportTargetType::Flock,
        "comment" => ReportTargetType::Comment,
        "user" => ReportTargetType::User,
        _ => ReportTargetType::Skill,
    }
}

fn reason_to_str(value: ReportReason) -> &'static str {
    match value {
        ReportReason::Spam => "spam",
        ReportReason::Abuse => "abuse",
        ReportReason::Inappropriate => "inappropriate",
        ReportReason::Copyright => "copyright",
        ReportReason::Other => "other",
    }
}

fn parse_reason(value: &str) -> ReportReason {
    match value {
        "spam" => ReportReason::Spam,
        "abuse" => ReportReason::Abuse,
        "inappropriate" => ReportReason::Inappropriate,
        "copyright" => ReportReason::Copyright,
        _ => ReportReason::Other,
    }
}

fn report_status_to_str(value: ReportStatus) -> &'static str {
    match value {
        ReportStatus::Pending => "pending",
        ReportStatus::Reviewed => "reviewed",
        ReportStatus::Resolved => "resolved",
        ReportStatus::Dismissed => "dismissed",
    }
}

fn parse_report_status(value: &str) -> ReportStatus {
    match value {
        "reviewed" => ReportStatus::Reviewed,
        "resolved" => ReportStatus::Resolved,
        "dismissed" => ReportStatus::Dismissed,
        _ => ReportStatus::Pending,
    }
}
