use futures_util::StreamExt;
use salvo::Depot;
use salvo::http::header::{self, HeaderValue};
use salvo::prelude::*;
use salvo::serve_static::StaticDir;
use salvo::websocket::WebSocketUpgrade;
use serde_json::json;
use shared::{
    AddSiteAdminRequest, BanUserRequest, CreateCommentRequest, CreateIndexRuleRequest,
    CreateRepoRequest, CreateReportRequest, ModerationUpdateRequest, RateFlockRequest,
    RecordInstallRequest, RecordViewRequest, ResourceKind, ReviewReportRequest,
    SaveCustomSelectorsRequest, SetUserRoleRequest, SubmitIndexRequest, UpdateIndexRuleRequest,
    UpdateSecurityStatusRequest,
};
use uuid::Uuid;

use crate::auth::{AuthContext, optional_auth, require_auth};
use crate::error::AppError;
use crate::service::{
    admin, blocks, browse_history, catalog, custom_selectors, github_auth, index_jobs,
    index_rules_admin, interactions, official_selectors, reports, repos, security, site_admins,
    users,
};
use crate::state::app_state;

pub fn router() -> Router {
    Router::new().push(
        Router::with_path("api/v1")
            // ── Group 1: Public (no auth) ──────────────────────────────
            .push(Router::with_path("health").get(health))
            .push(Router::with_path("ws").goal(ws_events))
            .push(Router::with_path("auth/github/start").get(start_github_login))
            .push(Router::with_path("auth/github/callback").get(finish_github_login))
            .push(Router::with_path("whoami").get(whoami))
            .push(Router::with_path("search").get(search))
            .push(Router::with_path("resolve").get(resolve_skill))
            .push(Router::with_path("download").get(download_bundle))
            .push(
                Router::with_path("skills")
                    .get(list_skills)
                    .push(
                        Router::with_path("{id}")
                            .get(get_skill_detail)
                            .push(Router::with_path("file").get(get_skill_file)),
                    ),
            )
            .push(Router::with_path("collect").post(collect_install))
            .push(
                Router::with_path("flocks")
                    .get(list_all_flocks)
                    .push(Router::with_path("{id}").get(get_flock_by_id)),
            )
            .push(
                Router::with_path("repos")
                    .get(list_repos)
                    .push(
                        Router::with_path("{domain}/{owner}/{name}")
                            .get(get_repo_detail)
                            .push(Router::with_path("flocks").push(
                                Router::with_path("{flock_slug}")
                                    .get(get_flock_detail)
                                    .push(Router::with_path("scans").get(list_flock_scans)),
                            )),
                    ),
            )
            .push(
                Router::with_path("users")
                    .get(list_users)
                    .push(
                        Router::with_path("{handle}")
                            .get(get_user_profile)
                            .push(Router::with_path("published-skills").get(get_user_published_skills))
                            .push(Router::with_path("starred-skills").get(get_user_starred_skills))
                            .push(Router::with_path("starred-flocks").get(get_user_starred_flocks))
                            .push(Router::with_path("history").get(get_user_history)),
                    ),
            )
            .push(
                Router::with_path("docs/{lang}")
                    .get(get_doc_page)
                    .push(Router::with_path("{**path}").get(get_doc_page)),
            )
            .push(Router::with_path("selectors/official").get(get_official_selectors))
            // ── Group 2: Login required ────────────────────────────────
            .push(
                Router::new()
                    .hoop(login_hoop)
                    .push(
                        Router::with_path("index")
                            .post(submit_index)
                            .push(Router::with_path("list").get(list_index_jobs))
                            .push(Router::with_path("{id}").get(get_index_job)),
                    )
                    .push(Router::with_path("repos").post(create_repo).push(
                        Router::with_path("{domain}/{owner}/{name}/flocks/{flock_slug}")
                            .push(
                                Router::with_path("comments")
                                    .post(add_flock_comment)
                                    .push(
                                        Router::with_path("{comment_id}")
                                            .delete(delete_flock_comment),
                                    ),
                            )
                            .push(Router::with_path("rate").post(rate_flock))
                            .push(Router::with_path("star").post(toggle_flock_star))
                            .push(
                                Router::with_path("block")
                                    .post(block_flock)
                                    .delete(unblock_flock),
                            )
                            .push(
                                Router::with_path("security")
                                    .post(update_flock_security)
                                    .push(
                                        Router::with_path("{skill_slug}")
                                            .post(update_skill_security),
                                    ),
                            ),
                    ))
                    .push(
                        Router::with_path("skills/{id}")
                            .delete(delete_skill)
                            .push(Router::with_path("restore").post(restore_skill))
                            .push(
                                Router::with_path("comments")
                                    .post(add_skill_comment)
                                    .push(
                                        Router::with_path("{comment_id}")
                                            .delete(delete_skill_comment),
                                    ),
                            )
                            .push(Router::with_path("star").post(toggle_skill_star))
                            .push(
                                Router::with_path("moderation").post(update_skill_moderation),
                            ),
                    )
                    .push(
                        Router::with_path("reports")
                            .get(list_reports)
                            .post(create_report)
                            .push(Router::with_path("{id}/review").post(review_report)),
                    )
                    .push(Router::with_path("blocks/flocks").get(list_blocked_flocks))
                    .push(
                        Router::with_path("me/selectors/custom")
                            .get(get_my_custom_selectors)
                            .post(save_my_custom_selectors)
                            .push(
                                Router::with_path("validate")
                                    .post(validate_my_custom_selectors),
                            ),
                    )
                    .push(Router::with_path("me/starred-skill-ids").get(get_my_starred_skill_ids))
                    .push(Router::with_path("me/stars").get(get_my_starred_skills))
                    .push(
                        Router::with_path("history")
                            .get(get_browse_history)
                            .post(record_view),
                    ),
            )
            // ── Group 3: Admin / staff required ────────────────────────
            .push(
                Router::with_path("management")
                    .hoop(login_hoop)
                    .hoop(staff_hoop)
                    .push(Router::with_path("summary").get(management_summary))
                    .push(Router::with_path("users/{id}/role").post(update_user_role))
                    .push(Router::with_path("users/{id}/ban").post(ban_user))
                    .push(
                        Router::with_path("site-admins")
                            .get(list_site_admins)
                            .post(add_site_admin)
                            .push(Router::with_path("{id}").delete(remove_site_admin)),
                    )
                    .push(
                        Router::with_path("index-rules")
                            .get(list_index_rules)
                            .post(create_index_rule)
                            .push(
                                Router::with_path("{id}")
                                    .post(update_index_rule)
                                    .delete(delete_index_rule),
                            ),
                    )
                    .push(
                        Router::with_path("jobs")
                            .get(admin_list_all_jobs)
                            .push(Router::with_path("{id}/cancel").post(admin_cancel_job)),
                    )
                    .push(Router::with_path("repos/{id}").delete(admin_delete_repo)),
            ),
    )
    // ── Static frontend assets (catch-all) ──────────────────────────
    .push(
        Router::with_path("{**rest}").get(
            StaticDir::new(["static"])
                .defaults("index.html")
                .fallback("index.html"),
        ),
    )
}

#[handler]
async fn health(res: &mut Response) {
    let state = crate::state::app_state();
    match state.pool.get() {
        Ok(_) => res.render(Json(json!({ "status": "ok" }))),
        Err(_) => {
            res.status_code(StatusCode::SERVICE_UNAVAILABLE);
            res.render(Json(
                json!({ "status": "unhealthy", "error": "database unavailable" }),
            ));
        }
    }
}

#[handler]
async fn get_official_selectors(req: &mut Request, res: &mut Response) {
    let if_none_match = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    match official_selectors::get_official_selectors(if_none_match) {
        Some((json, etag)) => {
            res.headers_mut().insert(
                header::ETAG,
                HeaderValue::from_str(etag).unwrap_or_else(|_| HeaderValue::from_static("")),
            );
            res.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            );
            res.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            res.write_body(json.to_string()).ok();
        }
        None => {
            res.status_code(StatusCode::NOT_MODIFIED);
        }
    }
}

#[handler]
async fn start_github_login(req: &mut Request, res: &mut Response) {
    let return_to = req.query::<String>("return_to");
    tracing::info!("GitHub login started, return_to={:?}", return_to);
    match github_auth::start_login(return_to.as_deref()) {
        Ok(redirect) => render_redirect(res, redirect),
        Err(error) => {
            tracing::warn!("GitHub login start failed: {}", error.message());
            render_error(res, error);
        }
    }
}

#[handler]
async fn finish_github_login(req: &mut Request, res: &mut Response) {
    let cookie_header = req
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let code = req.query::<String>("code").unwrap_or_default();
    let state = req.query::<String>("state").unwrap_or_default();

    if code.is_empty() || state.is_empty() {
        render_redirect(
            res,
            github_auth::error_redirect(
                cookie_header.as_deref(),
                "GitHub did not return the expected login callback parameters",
            ),
        );
        return;
    }

    match github_auth::finish_login(&code, &state, cookie_header.as_deref()).await {
        Ok(redirect) => {
            tracing::info!("GitHub login completed successfully");
            render_redirect(res, redirect);
        }
        Err(error) => {
            tracing::warn!("GitHub login failed: {}", error.message());
            render_redirect(
                res,
                github_auth::error_redirect(cookie_header.as_deref(), error.message()),
            );
        }
    }
}

#[handler]
async fn whoami(req: &mut Request, res: &mut Response) {
    match optional_auth(req) {
        Ok(auth) => {
            let response = catalog::whoami(auth.as_ref());
            if let Some(user) = response.user.as_ref() {
                tracing::debug!("whoami: {} ({:?})", user.handle, user.role);
            } else {
                tracing::debug!("whoami: anonymous");
            }
            res.render(Json(response));
        }
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn search(req: &mut Request, res: &mut Response) {
    let query = req.query::<String>("q").unwrap_or_default();
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let kind = req
        .query::<String>("kind")
        .and_then(|kind| parse_kind(&kind));
    match catalog::search_catalog(&query, kind, limit) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn resolve_skill(req: &mut Request, res: &mut Response) {
    let slug = req.query::<String>("slug").unwrap_or_default();
    let hash = req.query::<String>("hash").unwrap_or_default();
    if slug.is_empty() || hash.is_empty() {
        render_error(
            res,
            AppError::BadRequest("slug and hash query parameters are required".to_string()),
        );
        return;
    }
    match catalog::resolve_skill(&slug, &hash) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn download_bundle(req: &mut Request, res: &mut Response) {
    let slug = req.query::<String>("slug").unwrap_or_default();
    let version = req.query::<String>("version");
    let tag = req.query::<String>("tag");
    let kind = req
        .query::<String>("kind")
        .and_then(|value| parse_kind(&value));
    let auth = optional_auth(req).ok().flatten();
    let result = match kind.unwrap_or(ResourceKind::Skill) {
        ResourceKind::Skill => catalog::download_skill_bundle(
            &slug,
            version.as_deref(),
            tag.as_deref(),
            auth.as_ref().map(|ctx| &ctx.user),
        ),
    };

    match result {
        Ok(bundle) => {
            let _ = res.add_header(
                header::CONTENT_TYPE,
                HeaderValue::from_str(&bundle.content_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
                true,
            );
            let _ = res.add_header(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{}\"", bundle.filename))
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
                true,
            );
            let _ = res.write_body(bundle.bytes);
        }
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_skills(req: &mut Request, res: &mut Response) {
    let sort = req
        .query::<String>("sort")
        .unwrap_or_else(|| "updated".to_string());
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let cursor = req.query::<String>("cursor");
    let q = req.query::<String>("q");
    let repo_param = req.query::<String>("repo");
    let path_param = req.query::<String>("path");
    let repo_id = repo_param
        .as_deref()
        .and_then(|v| uuid::Uuid::parse_str(v).ok());
    let repo_url = repo_param.filter(|v| repo_id.is_none() && !v.trim().is_empty());
    let flock_id = req
        .query::<String>("flock")
        .and_then(|v| uuid::Uuid::parse_str(&v).ok());
    let auth = optional_auth(req).ok().flatten();
    match catalog::list_skills(
        &sort,
        limit,
        cursor,
        q,
        repo_id,
        repo_url,
        path_param,
        flock_id,
        auth.as_ref().map(|ctx| &ctx.user),
    ) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_all_flocks(req: &mut Request, res: &mut Response) {
    let sort = req
        .query::<String>("sort")
        .unwrap_or_else(|| "updated".to_string());
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let cursor = req.query::<String>("cursor");
    let q = req.query::<String>("q");
    let repo_param = req.query::<String>("repo");
    let slug_param = req.query::<String>("slug");
    let repo_id = repo_param
        .as_deref()
        .and_then(|v| uuid::Uuid::parse_str(v).ok());
    let repo_url = repo_param.filter(|v| repo_id.is_none() && !v.trim().is_empty());
    match catalog::list_flocks(&sort, limit, cursor, q, repo_id, repo_url, slug_param) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_flock_by_id(req: &mut Request, res: &mut Response) {
    let id = match parse_uuid_param(req, "id", "invalid flock id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let auth = optional_auth(req).ok().flatten();
    match repos::get_flock_by_id(id, auth.as_ref().map(|ctx| &ctx.user)) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_repos(req: &mut Request, res: &mut Response) {
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let cursor = req.query::<String>("cursor");
    let q = req.query::<String>("q");
    match repos::list_repos(limit, cursor, q) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn create_repo(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<CreateRepoRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match repos::create_repo(auth, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_repo_detail(req: &mut Request, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    match repos::get_repo_detail(&domain, &path_slug) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_flock_detail(req: &mut Request, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    let auth = optional_auth(req).ok().flatten();
    match repos::get_flock_detail(
        &domain,
        &path_slug,
        &flock_slug,
        auth.as_ref().map(|ctx| &ctx.user),
    ) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_skill_detail(req: &mut Request, res: &mut Response) {
    let id = match parse_uuid_param(req, "id", "invalid skill id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let auth = optional_auth(req).ok().flatten();
    match catalog::get_skill_detail_by_id(id, auth.as_ref().map(|ctx| &ctx.user)) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_skill_file(req: &mut Request, res: &mut Response) {
    let id = match parse_uuid_param(req, "id", "invalid skill id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let path = req.query::<String>("path").unwrap_or_default();
    let version = req.query::<String>("version");
    let tag = req.query::<String>("tag");
    let auth = optional_auth(req).ok().flatten();
    match catalog::get_skill_file_by_id(
        id,
        version.as_deref(),
        tag.as_deref(),
        &path,
        auth.as_ref().map(|ctx| &ctx.user),
    ) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn add_skill_comment(req: &mut Request, depot: &Depot, res: &mut Response) {
    let id = match parse_uuid_param(req, "id", "invalid skill id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<CreateCommentRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match interactions::add_skill_comment(auth, id, body) {
        Ok(payload) => res.render(Json(json!({ "comments": payload }))),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn delete_skill_comment(req: &mut Request, depot: &Depot, res: &mut Response) {
    let id = match parse_uuid_param(req, "id", "invalid skill id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let comment_id = match parse_uuid_param(req, "comment_id", "invalid comment id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let auth = auth_from_depot(depot);
    match interactions::delete_skill_comment(auth, id, comment_id) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn toggle_skill_star(req: &mut Request, depot: &Depot, res: &mut Response) {
    let id = match parse_uuid_param(req, "id", "invalid skill id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let auth = auth_from_depot(depot);
    match interactions::toggle_skill_star(auth, id) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn toggle_flock_star(req: &mut Request, depot: &Depot, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    let auth = auth_from_depot(depot);
    match interactions::toggle_flock_star(auth, &domain, &path_slug, &flock_slug) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn update_skill_moderation(req: &mut Request, depot: &Depot, res: &mut Response) {
    let id = match parse_uuid_param(req, "id", "invalid skill id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<ModerationUpdateRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match admin::update_skill_moderation(auth, id, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn delete_skill(req: &mut Request, depot: &Depot, res: &mut Response) {
    let id = match parse_uuid_param(req, "id", "invalid skill id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let auth = auth_from_depot(depot);
    match admin::set_skill_deleted(auth, id, true) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn restore_skill(req: &mut Request, depot: &Depot, res: &mut Response) {
    let id = match parse_uuid_param(req, "id", "invalid skill id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let auth = auth_from_depot(depot);
    match admin::set_skill_deleted(auth, id, false) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn collect_install(req: &mut Request, res: &mut Response) {
    let repo_url = req.query::<String>("repo").unwrap_or_default();
    let skill_path = req.query::<String>("path").unwrap_or_default();
    if repo_url.is_empty() || skill_path.is_empty() {
        render_error(
            res,
            AppError::BadRequest("repo and path query parameters are required".to_string()),
        );
        return;
    }
    let body: RecordInstallRequest = req.parse_json().await.unwrap_or(RecordInstallRequest {
        client_type: "unknown".to_string(),
    });
    let auth = optional_auth(req).ok().flatten();
    let user_id = auth.map(|a| a.user.id);
    let client_type = if body.client_type.is_empty() {
        "unknown"
    } else {
        &body.client_type
    };
    match interactions::record_skill_install(&repo_url, &skill_path, user_id, client_type) {
        Ok(result) => res.render(Json(json!({ "ok": result.ok, "message": result.message }))),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_users(req: &mut Request, res: &mut Response) {
    let q = req.query::<String>("q");
    let limit = req.query::<i64>("limit").unwrap_or(30);
    match users::list_users(q.as_deref(), limit) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_user_profile(req: &mut Request, res: &mut Response) {
    let handle = req.param::<String>("handle").unwrap_or_default();
    let auth = optional_auth(req).ok().flatten();
    match users::get_user_profile(&handle, auth.as_ref().map(|ctx| &ctx.user)) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_user_published_skills(req: &mut Request, res: &mut Response) {
    let handle = req.param::<String>("handle").unwrap_or_default();
    let auth = optional_auth(req).ok().flatten();
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let cursor = req.query::<String>("cursor");
    match users::get_user_published_skills(
        &handle,
        auth.as_ref().map(|ctx| &ctx.user),
        limit,
        cursor,
    ) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_user_starred_skills(req: &mut Request, res: &mut Response) {
    let handle = req.param::<String>("handle").unwrap_or_default();
    let auth = optional_auth(req).ok().flatten();
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let cursor = req.query::<String>("cursor");
    match users::get_user_starred_skills(
        &handle,
        auth.as_ref().map(|ctx| &ctx.user),
        limit,
        cursor,
    ) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_user_starred_flocks(req: &mut Request, res: &mut Response) {
    let handle = req.param::<String>("handle").unwrap_or_default();
    let auth = optional_auth(req).ok().flatten();
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let cursor = req.query::<String>("cursor");
    match users::get_user_starred_flocks(
        &handle,
        auth.as_ref().map(|ctx| &ctx.user),
        limit,
        cursor,
    ) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_user_history(req: &mut Request, res: &mut Response) {
    let handle = req.param::<String>("handle").unwrap_or_default();
    let auth = optional_auth(req).ok().flatten();
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let cursor = req.query::<String>("cursor");
    match users::get_user_history(&handle, auth.as_ref().map(|ctx| &ctx.user), limit, cursor) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn management_summary(depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    match admin::management_summary(auth) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn update_user_role(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let id = match req
        .param::<String>("id")
        .and_then(|value| Uuid::parse_str(&value).ok())
    {
        Some(id) => id,
        None => {
            render_error(res, AppError::BadRequest("invalid user id".to_string()));
            return;
        }
    };
    let body = match req.parse_json::<SetUserRoleRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match admin::set_user_role(auth, id, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn ban_user(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let id = match req
        .param::<String>("id")
        .and_then(|value| Uuid::parse_str(&value).ok())
    {
        Some(id) => id,
        None => {
            render_error(res, AppError::BadRequest("invalid user id".to_string()));
            return;
        }
    };
    let body = match req.parse_json::<BanUserRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match admin::ban_user(auth, id, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

/// Reconstruct `path_slug` from the `{owner}/{name}` route segments.
fn repo_path_slug(req: &Request) -> String {
    let owner = req.param::<String>("owner").unwrap_or_default();
    let name = req.param::<String>("name").unwrap_or_default();
    format!("{owner}/{name}")
}

fn parse_uuid_param(req: &Request, name: &str, error_message: &str) -> Result<Uuid, AppError> {
    req.param::<String>(name)
        .and_then(|value| Uuid::parse_str(&value).ok())
        .ok_or_else(|| AppError::BadRequest(error_message.to_string()))
}

#[handler]
async fn add_flock_comment(req: &mut Request, depot: &Depot, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<CreateCommentRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match interactions::add_flock_comment(auth, &domain, &path_slug, &flock_slug, body) {
        Ok(payload) => res.render(Json(json!({ "comments": payload }))),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn delete_flock_comment(req: &mut Request, depot: &Depot, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    let comment_id = match parse_uuid_param(req, "comment_id", "invalid comment id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let auth = auth_from_depot(depot);
    match interactions::delete_flock_comment(auth, &domain, &path_slug, &flock_slug, comment_id) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn rate_flock(req: &mut Request, depot: &Depot, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<RateFlockRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match interactions::rate_flock(auth, &domain, &path_slug, &flock_slug, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn create_report(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<CreateReportRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match reports::create_report(auth, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_reports(depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    match reports::list_reports(auth) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn review_report(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let id = match req
        .param::<String>("id")
        .and_then(|value| Uuid::parse_str(&value).ok())
    {
        Some(id) => id,
        None => {
            render_error(res, AppError::BadRequest("invalid report id".to_string()));
            return;
        }
    };
    let body = match req.parse_json::<ReviewReportRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match reports::review_report(auth, id, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn block_flock(req: &mut Request, depot: &Depot, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    let auth = auth_from_depot(depot);
    match blocks::block_flock(auth, &domain, &path_slug, &flock_slug) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn unblock_flock(req: &mut Request, depot: &Depot, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    let auth = auth_from_depot(depot);
    match blocks::unblock_flock(auth, &domain, &path_slug, &flock_slug) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_blocked_flocks(depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    match blocks::list_blocked_flocks(auth) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_my_custom_selectors(depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    match custom_selectors::get_custom_selectors(auth) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

/// D5: POST /me/selectors/custom/validate — dry-run validation, no persist.
#[handler]
async fn validate_my_custom_selectors(req: &mut Request, _depot: &Depot, res: &mut Response) {
    let body = match req.parse_json::<shared::SaveCustomSelectorsRequest>().await {
        Ok(body) => body,
        Err(error) => {
            return render_error(res, AppError::BadRequest(error.to_string()));
        }
    };
    let response = custom_selectors::validate_custom_selectors(body);
    res.render(Json(response));
}

#[handler]
async fn save_my_custom_selectors(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<SaveCustomSelectorsRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match custom_selectors::save_custom_selectors(auth, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_my_starred_skill_ids(depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    match interactions::get_starred_skill_ids(auth) {
        Ok(ids) => res.render(Json(shared::StarredIdsResponse { skill_ids: ids })),
        Err(error) => render_error(res, error),
    }
}

/// D2: GET /me/stars — paged list of skills the caller has starred,
/// newest-first. Optional `limit` query param, clamped to [1, 100].
#[handler]
async fn get_my_starred_skills(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let limit = req.query::<i64>("limit").unwrap_or(50);
    match catalog::list_my_starred_skills(auth, limit) {
        Ok(page) => res.render(Json(page)),
        Err(error) => render_error(res, error),
    }
}

fn parse_kind(value: &str) -> Option<ResourceKind> {
    match value {
        "skill" | "skills" => Some(ResourceKind::Skill),
        _ => None,
    }
}

fn render_error(res: &mut Response, error: AppError) {
    res.status_code(error.status_code());
    res.render(Json(json!({ "error": error.message() })));
}

/// Hoop: reject requests without a valid bearer token.
#[handler]
async fn login_hoop(req: &mut Request, depot: &mut Depot, res: &mut Response, ctrl: &mut FlowCtrl) {
    match require_auth(req) {
        Ok(auth) => {
            depot.insert("auth_context", auth);
        }
        Err(error) => {
            render_error(res, error);
            ctrl.skip_rest();
        }
    }
}

/// Hoop: reject requests from non-staff users (must be placed after `login_hoop`).
#[handler]
async fn staff_hoop(depot: &mut Depot, res: &mut Response, ctrl: &mut FlowCtrl) {
    let is_staff = depot.get::<AuthContext>("auth_context").is_ok_and(|a| {
        matches!(
            a.user.role,
            shared::UserRole::Admin | shared::UserRole::Moderator
        )
    });
    if !is_staff {
        render_error(
            res,
            AppError::Forbidden("admin access required".to_string()),
        );
        ctrl.skip_rest();
    }
}

/// Extract the `AuthContext` that was stored by `login_hoop`.
fn auth_from_depot(depot: &Depot) -> &AuthContext {
    depot
        .get::<AuthContext>("auth_context")
        .expect("auth_context missing from depot – login_hoop not applied?")
}

#[handler]
async fn submit_index(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<SubmitIndexRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match index_jobs::submit_index(auth, body).await {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_index_job(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let id = match req
        .param::<String>("id")
        .and_then(|value| Uuid::parse_str(&value).ok())
    {
        Some(id) => id,
        None => {
            render_error(res, AppError::BadRequest("invalid job id".to_string()));
            return;
        }
    };
    match index_jobs::get_index_job(auth, id) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_index_jobs(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let offset = req.query::<i64>("offset").unwrap_or(0);
    match index_jobs::list_index_jobs(auth, limit, offset) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn update_flock_security(req: &mut Request, depot: &Depot, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<UpdateSecurityStatusRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match security::update_flock_security_status(auth, &domain, &path_slug, &flock_slug, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn update_skill_security(req: &mut Request, depot: &Depot, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    let skill_slug = req.param::<String>("skill_slug").unwrap_or_default();
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<UpdateSecurityStatusRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match security::update_skill_security_status(
        auth,
        &domain,
        &path_slug,
        &flock_slug,
        &skill_slug,
        body,
    ) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_flock_scans(req: &mut Request, res: &mut Response) {
    let domain = req.param::<String>("domain").unwrap_or_default();
    let path_slug = repo_path_slug(req);
    let flock_slug = req.param::<String>("flock_slug").unwrap_or_default();
    match security::list_flock_scans_by_slugs(&domain, &path_slug, &flock_slug) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_site_admins(depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    match site_admins::list_site_admins(auth) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn add_site_admin(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<AddSiteAdminRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match site_admins::add_site_admin(auth, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn remove_site_admin(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let id = match req
        .param::<String>("id")
        .and_then(|value| Uuid::parse_str(&value).ok())
    {
        Some(id) => id,
        None => {
            render_error(res, AppError::BadRequest("invalid user id".to_string()));
            return;
        }
    };
    match site_admins::remove_site_admin(auth, id) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn list_index_rules(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let q = req.query::<String>("q");
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let cursor = req.query::<String>("cursor");
    match index_rules_admin::list_index_rules(auth, q.as_deref(), limit, cursor) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn create_index_rule(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<CreateIndexRuleRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match index_rules_admin::create_index_rule(auth, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn update_index_rule(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let id = match parse_uuid_param(req, "id", "invalid index rule id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    let body = match req.parse_json::<UpdateIndexRuleRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match index_rules_admin::update_index_rule(auth, id, body) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn delete_index_rule(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let id = match parse_uuid_param(req, "id", "invalid index rule id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    match index_rules_admin::delete_index_rule(auth, id) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn admin_list_all_jobs(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let q = req.query::<String>("q");
    let limit = req.query::<i64>("limit").unwrap_or(20);
    let cursor = req.query::<String>("cursor");
    match admin::list_all_index_jobs(auth, q.as_deref(), limit, cursor) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn admin_cancel_job(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let id = match parse_uuid_param(req, "id", "invalid job id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    match admin::cancel_index_job(auth, id) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn admin_delete_repo(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let id = match parse_uuid_param(req, "id", "invalid repo id") {
        Ok(id) => id,
        Err(error) => {
            render_error(res, error);
            return;
        }
    };
    match admin::delete_repo(auth, id) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_doc_page(req: &mut Request, res: &mut Response) {
    let lang = req
        .param::<String>("lang")
        .unwrap_or_else(|| "en".to_string());
    let path = req.param::<String>("path").unwrap_or_default();
    let route = if path.is_empty() {
        "/".to_string()
    } else {
        path
    };
    match crate::docs::get_page(&lang, &route) {
        Some(page) => res.render(Json(page)),
        None => render_error(res, AppError::NotFound("doc page not found".to_string())),
    }
}

fn render_redirect(res: &mut Response, redirect: github_auth::AuthRedirect) {
    res.status_code(StatusCode::FOUND);
    if let Ok(location) = HeaderValue::from_str(&redirect.location) {
        let _ = res.add_header(header::LOCATION, location, true);
    }
    for value in redirect.set_cookies {
        if let Ok(cookie) = HeaderValue::from_str(&value) {
            let _ = res.add_header(header::SET_COOKIE, cookie, false);
        }
    }
}

#[derive(serde::Deserialize)]
struct WsClientMessage {
    action: String,
    job_id: Option<String>,
}

// Heartbeat tunables for /ws.
//
// Server sends a Ping every PING_INTERVAL. If no frame of any kind (ping
// reply, text, binary) is observed for IDLE_TIMEOUT, the connection is
// closed so a half-open TCP socket can't pin a worker forever.
const WS_PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);
const WS_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[handler]
async fn ws_events(req: &mut Request, res: &mut Response) -> Result<(), StatusError> {
    WebSocketUpgrade::new()
        .upgrade(req, res, |mut ws| async move {
            let mut rx = app_state().events_tx.subscribe();
            let mut subscriptions = std::collections::HashSet::<String>::new();
            let mut ping_ticker = tokio::time::interval(WS_PING_INTERVAL);
            // Skip the immediate first tick so we don't ping before the
            // client has had a chance to settle.
            ping_ticker.tick().await;
            let mut last_seen = tokio::time::Instant::now();
            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Ok(ref evt) => {
                                let job_id_str = match evt {
                                    crate::state::WsEvent::IndexProgress { job_id, .. } => job_id.to_string(),
                                };
                                if subscriptions.contains(&job_id_str)
                                    && let Ok(text) = serde_json::to_string(evt)
                                        && ws.send(salvo::websocket::Message::text(text)).await.is_err() {
                                            break;
                                        }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => break,
                        }
                    }
                    msg = ws.next() => {
                        match msg {
                            Some(Ok(msg)) => {
                                last_seen = tokio::time::Instant::now();
                                if msg.is_ping() {
                                    // Reply with pong echoing the payload.
                                    let payload = msg.as_bytes().to_vec();
                                    if ws.send(salvo::websocket::Message::pong(payload)).await.is_err() {
                                        break;
                                    }
                                } else if msg.is_pong() {
                                    // Heartbeat reply — last_seen already updated.
                                } else if msg.is_close() {
                                    break;
                                } else if msg.is_text()
                                    && let Ok(text) = msg.as_str()
                                        && let Ok(cmd) = serde_json::from_str::<WsClientMessage>(text) {
                                            match cmd.action.as_str() {
                                                "subscribe" => {
                                                    if let Some(id) = cmd.job_id {
                                                        subscriptions.insert(id);
                                                    }
                                                }
                                                "unsubscribe" => {
                                                    if let Some(id) = cmd.job_id {
                                                        subscriptions.remove(&id);
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                            }
                            _ => break,
                        }
                    }
                    _ = ping_ticker.tick() => {
                        // Drop dead connections proactively.
                        if last_seen.elapsed() > WS_IDLE_TIMEOUT {
                            tracing::debug!("ws idle > {:?}, closing", WS_IDLE_TIMEOUT);
                            break;
                        }
                        if ws.send(salvo::websocket::Message::ping(Vec::new())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .await
}

#[handler]
async fn record_view(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let body = match req.parse_json::<RecordViewRequest>().await {
        Ok(body) => body,
        Err(error) => {
            render_error(res, AppError::BadRequest(error.to_string()));
            return;
        }
    };
    match browse_history::record_view(auth, body) {
        Ok(()) => res.render(Json(json!({ "ok": true }))),
        Err(error) => render_error(res, error),
    }
}

#[handler]
async fn get_browse_history(req: &mut Request, depot: &Depot, res: &mut Response) {
    let auth = auth_from_depot(depot);
    let limit = req.query::<i64>("limit").unwrap_or(50);
    match browse_history::get_history(auth, limit) {
        Ok(payload) => res.render(Json(payload)),
        Err(error) => render_error(res, error),
    }
}
