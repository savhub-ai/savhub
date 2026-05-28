use chrono::Utc;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::{Value, json};
use shared::{
    CustomSelectorsResponse, SaveCustomSelectorsRequest, SelectorValidationIssue,
    ValidateCustomSelectorsResponse,
};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::models::{NewUserCustomSelectorsRow, UserCustomSelectorsRow};
use crate::schema::user_custom_selectors;
use crate::service::helpers::{db_conn, insert_audit_log};

/// Return the authenticated user's custom selectors.
pub async fn get_custom_selectors(auth: &AuthContext) -> Result<CustomSelectorsResponse, AppError> {
    let mut conn = db_conn().await?;
    let row = user_custom_selectors::table
        .filter(user_custom_selectors::user_id.eq(auth.user.id))
        .select(UserCustomSelectorsRow::as_select())
        .first::<UserCustomSelectorsRow>(&mut conn)
        .await
        .optional()?;

    match row {
        Some(row) => {
            let selectors = row.selectors.as_array().cloned().unwrap_or_default();
            Ok(CustomSelectorsResponse {
                ok: true,
                selectors,
                version: row.version as u8,
                updated_at: Some(row.updated_at),
            })
        }
        None => Ok(CustomSelectorsResponse {
            ok: true,
            selectors: Vec::new(),
            version: 1,
            updated_at: None,
        }),
    }
}

/// Maximum number of custom selector entries a single user may store.
/// Keeps the DB row size bounded and prevents accidental/abusive bulk uploads.
const MAX_CUSTOM_SELECTORS_PER_USER: usize = 200;

/// Upsert the authenticated user's custom selectors.
pub async fn save_custom_selectors(
    auth: &AuthContext,
    request: SaveCustomSelectorsRequest,
) -> Result<CustomSelectorsResponse, AppError> {
    if request.selectors.len() > MAX_CUSTOM_SELECTORS_PER_USER {
        return Err(AppError::BadRequest(format!(
            "too many selectors: {} (max {MAX_CUSTOM_SELECTORS_PER_USER})",
            request.selectors.len()
        )));
    }

    let mut conn = db_conn().await?;
    let now = Utc::now();
    let selectors_json = Value::Array(request.selectors.clone());

    let existing = user_custom_selectors::table
        .filter(user_custom_selectors::user_id.eq(auth.user.id))
        .select(UserCustomSelectorsRow::as_select())
        .first::<UserCustomSelectorsRow>(&mut conn)
        .await
        .optional()?;

    let row_id = if let Some(existing) = existing {
        diesel::update(user_custom_selectors::table.find(existing.id))
            .set((
                user_custom_selectors::selectors.eq(&selectors_json),
                user_custom_selectors::version.eq(request.version as i16),
                user_custom_selectors::updated_at.eq(now),
            ))
            .execute(&mut conn)
            .await?;
        existing.id
    } else {
        let id = Uuid::now_v7();
        diesel::insert_into(user_custom_selectors::table)
            .values(NewUserCustomSelectorsRow {
                id,
                user_id: auth.user.id,
                selectors: selectors_json,
                version: request.version as i16,
                updated_at: now,
                created_at: now,
            })
            .execute(&mut conn)
            .await?;
        id
    };

    insert_audit_log(
        &mut conn,
        Some(auth.user.id),
        "user.selectors.sync",
        "user_custom_selectors",
        Some(row_id),
        json!({ "count": request.selectors.len() }),
    )
    .await?;

    Ok(CustomSelectorsResponse {
        ok: true,
        selectors: request.selectors,
        version: request.version,
        updated_at: Some(now),
    })
}

/// D5: dry-run validate a custom selectors payload without persisting.
///
/// Surfaces shape errors per entry so the editor can show inline messages.
/// Validation is intentionally permissive: it checks the keys the indexer and
/// CLI rely on (`name`, `kind`, and a non-empty pattern field appropriate for
/// the kind) without locking in a specific DSL.
pub fn validate_custom_selectors(
    request: SaveCustomSelectorsRequest,
) -> ValidateCustomSelectorsResponse {
    const KNOWN_KINDS: &[&str] = &["file_glob", "dependency", "regex", "composite"];

    let mut issues: Vec<SelectorValidationIssue> = Vec::new();

    for (index, entry) in request.selectors.iter().enumerate() {
        let Some(obj) = entry.as_object() else {
            issues.push(SelectorValidationIssue {
                index,
                field: String::new(),
                message: "selector entry must be a JSON object".into(),
            });
            continue;
        };

        match obj.get("name").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => {}
            _ => issues.push(SelectorValidationIssue {
                index,
                field: "name".into(),
                message: "name is required and must be a non-empty string".into(),
            }),
        }

        let kind = obj.get("kind").and_then(|v| v.as_str());
        match kind {
            Some(k) if KNOWN_KINDS.contains(&k) => {}
            Some(other) => issues.push(SelectorValidationIssue {
                index,
                field: "kind".into(),
                message: format!("kind '{other}' is not one of {}", KNOWN_KINDS.join(", ")),
            }),
            None => issues.push(SelectorValidationIssue {
                index,
                field: "kind".into(),
                message: format!("kind is required (one of {})", KNOWN_KINDS.join(", ")),
            }),
        }

        // Pattern checks vary by kind. For composite we accept a `selectors`
        // array; everything else must carry a non-empty `pattern` string OR
        // a non-empty `patterns` array.
        if matches!(kind, Some("composite")) {
            match obj.get("selectors").and_then(|v| v.as_array()) {
                Some(arr) if !arr.is_empty() => {}
                _ => issues.push(SelectorValidationIssue {
                    index,
                    field: "selectors".into(),
                    message: "composite selector requires a non-empty `selectors` array".into(),
                }),
            }
        } else {
            let has_single = obj
                .get("pattern")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let has_many = obj
                .get("patterns")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            if !has_single && !has_many {
                issues.push(SelectorValidationIssue {
                    index,
                    field: "pattern".into(),
                    message: "pattern (string) or patterns (array) is required".into(),
                });
            }
        }
    }

    ValidateCustomSelectorsResponse {
        ok: issues.is_empty(),
        issues,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn req(selectors: Vec<Value>) -> SaveCustomSelectorsRequest {
        SaveCustomSelectorsRequest {
            selectors,
            version: 1,
        }
    }

    #[test]
    fn validate_accepts_well_formed_file_glob() {
        let r = validate_custom_selectors(req(vec![json!({
            "name": "rust-files",
            "kind": "file_glob",
            "pattern": "**/*.rs",
        })]));
        assert!(r.ok, "expected ok, got {:?}", r.issues);
    }

    #[test]
    fn validate_accepts_composite_with_children() {
        let r = validate_custom_selectors(req(vec![json!({
            "name": "fullstack",
            "kind": "composite",
            "selectors": ["rust-files", "ts-files"],
        })]));
        assert!(r.ok);
    }

    #[test]
    fn validate_rejects_missing_name_and_kind() {
        let r = validate_custom_selectors(req(vec![json!({"pattern": "*.rs"})]));
        assert!(!r.ok);
        let fields: Vec<_> = r.issues.iter().map(|i| i.field.as_str()).collect();
        assert!(fields.contains(&"name"));
        assert!(fields.contains(&"kind"));
    }

    #[test]
    fn validate_rejects_unknown_kind() {
        let r = validate_custom_selectors(req(vec![json!({
            "name": "x",
            "kind": "telepathy",
            "pattern": "*",
        })]));
        assert!(!r.ok);
        assert!(r.issues.iter().any(|i| i.field == "kind"));
    }

    #[test]
    fn validate_rejects_non_object_entry() {
        let r = validate_custom_selectors(req(vec![json!("just a string")]));
        assert!(!r.ok);
        assert_eq!(r.issues.len(), 1);
        assert_eq!(r.issues[0].field, "");
    }

    #[test]
    fn validate_rejects_empty_pattern() {
        let r = validate_custom_selectors(req(vec![json!({
            "name": "empty",
            "kind": "regex",
            "pattern": "   ",
        })]));
        assert!(!r.ok);
        assert!(r.issues.iter().any(|i| i.field == "pattern"));
    }

    #[test]
    fn validate_accepts_empty_payload() {
        let r = validate_custom_selectors(req(vec![]));
        assert!(r.ok);
        assert!(r.issues.is_empty());
    }
}
