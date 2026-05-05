use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::{
    AdminActionResponse, CreateCommentRequest, FlockDetailResponse, RecordViewRequest,
    ToggleStarResponse,
};

use crate::api;
use crate::app::{
    CommentsList, Route, render_copy_sign, render_security_badge, render_source_label, repo_route,
    short_repo_name, token_option, url_lang,
};
use crate::contexts::{ApiContext, I18nContext};
use crate::pages::skills_list::SkillFlockList;
use crate::pages::widgets::CommentComposer;
use crate::urls::derive_repo_slug;

#[component]
pub(crate) fn FlockPage(lang: String, id: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let token = api.token.read().clone();
    let mut message = use_signal(String::new);
    let mut comment_body = use_signal(String::new);
    let mut resource = use_resource(move || {
        let token = token.clone();
        let id = id.clone();
        async move {
            api::get_json::<FlockDetailResponse>(
                api.api_base,
                token_option(&token),
                &format!("/flocks/{id}"),
            )
            .await
        }
    });
    let comment_value = comment_body.read().clone();
    match &*resource.read_unchecked() {
        Some(Ok(payload)) => {
            let comments = payload.comments.clone();
            let links = payload
                .document
                .metadata
                .links
                .iter()
                .map(|(label, value)| (label.clone(), value.clone()))
                .collect::<Vec<_>>();
            let repo_slug = derive_repo_slug(&payload.flock.repo_url);
            let repo_slug_bc = repo_slug.clone();
            let repo_name_bc = short_repo_name(&payload.flock.repo_url).to_string();
            let star_repo = repo_slug.clone();
            let star_flock = payload.flock.slug.clone();
            let comment_repo = repo_slug.clone();
            let comment_flock = payload.flock.slug.clone();
            let delete_comment_repo = repo_slug.clone();
            let delete_comment_flock = payload.flock.slug.clone();
            let api_base = api.api_base;
            let token_signal = api.token;
            // Record browse history
            {
                let flock_id = payload.flock.id;
                let view_token = api.token.read().clone();
                spawn(async move {
                    if !view_token.trim().is_empty() {
                        let _ = api::post_json::<_, serde_json::Value>(
                            api_base,
                            token_option(&view_token),
                            "/history",
                            &RecordViewRequest {
                                resource_type: "flock".to_string(),
                                resource_id: flock_id,
                            },
                        )
                        .await;
                    }
                });
            }
            rsx! {
                document::Title { "{payload.flock.name} - {t.brand_name}" }
                section { class: "section detail",
                    nav { class: "breadcrumb",
                        Link { to: Route::ReposPage { lang: url_lang() }, "{t.nav_repos}" }
                        span { class: "breadcrumb-sep", "/" }
                        Link { to: repo_route(&repo_slug_bc), "{repo_name_bc}" }
                        span { class: "breadcrumb-sep", "/" }
                        span { "{payload.flock.name}" }
                    }
                    if !message.read().is_empty() {
                        p { class: "flash", "{message}" }
                    }
                    div { class: "detail-head",
                        div {
                            h1 { "{payload.flock.name}"
                                { render_security_badge(&payload.flock.security_status, t) }
                            }
                            { render_copy_sign(&payload.flock.repo_url, &payload.flock.slug) }
                        }
                        div { class: "detail-actions",
                            button {
                                class: "primary",
                                onclick: move |_| {
                                    let repo = star_repo.clone();
                                    let flock = star_flock.clone();
                                    let token = token_signal.read().clone();
                                    if token.trim().is_empty() {
                                        message.set(t.login_before_star.to_string());
                                        return;
                                    }
                                    spawn(async move {
                                        let result = api::post_empty::<ToggleStarResponse>(
                                            api_base,
                                            token_option(&token),
                                            &format!("/repos/{repo}/flocks/{flock}/star"),
                                        )
                                        .await;
                                        match result {
                                            Ok(r) => message.set(format!("{} stars", r.stars)),
                                            Err(error) => message.set(error),
                                        }
                                        resource.restart();
                                    });
                                },
                                if payload.starred { "{t.unstar}" } else { "{t.star}" }
                            }
                            span { class: "pill", "{payload.flock.stats_stars} {t.stars}" }
                        }
                    }
                    p { class: "summary detail-summary", "{payload.flock.description}" }
                    div { class: "meta-row wide",
                        if let Some(ref v) = payload.flock.version {
                            span { "v{v}" }
                        }
                        span { "{payload.flock.status:?}" }
                        span { "{payload.flock.skill_count} {t.indexed_skills}" }
                        span { "{payload.flock.stats_comments} {t.comments}" }
                        span { "{render_source_label(&payload.flock.source, t)}" }
                        span { "{payload.flock.license}" }
                    }
                    div { class: "detail-grid",
                        if !links.is_empty() {
                            div { class: "panel",
                                h2 { "{t.links}" }
                                ul { class: "dense-list",
                                    for (label, value) in links {
                                        li {
                                            span { "{label}: " }
                                            a { href: "{value}", target: "_blank", rel: "noreferrer", "{value}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    SkillFlockList {
                        title: t.indexed_skills.to_string(),
                        storage_prefix: format!("savhub.flock.{}", payload.flock.id),
                        flock_id: Some(payload.flock.id.to_string()),
                        hide_grouped: true,
                    }
                    section { class: "panel",
                        h2 { "{t.comments}" }
                        CommentComposer {
                            body: comment_value,
                            placeholder: t.comment_placeholder,
                            on_input: move |value| comment_body.set(value),
                            on_submit: move |_| {
                                let repo = comment_repo.clone();
                                let flock = comment_flock.clone();
                                let token = token_signal.read().clone();
                                let body = comment_body.read().trim().to_string();
                                if token.trim().is_empty() {
                                    message.set(t.login_before_comment.to_string());
                                    return;
                                }
                                if body.is_empty() {
                                    message.set(t.comment_body_required.to_string());
                                    return;
                                }
                                spawn(async move {
                                    let result = api::post_json::<CreateCommentRequest, serde_json::Value>(
                                        api_base,
                                        token_option(&token),
                                        &format!("/repos/{repo}/flocks/{flock}/comments"),
                                        &CreateCommentRequest { body },
                                    )
                                    .await;
                                    match result {
                                        Ok(_) => {
                                            comment_body.set(String::new());
                                            message.set(t.comment_posted.to_string());
                                            resource.restart();
                                        }
                                        Err(error) => message.set(error),
                                    }
                                });
                            }
                        }
                        CommentsList {
                            comments,
                            empty_label: t.no_comments_yet,
                            delete_label: t.delete_comment,
                            on_delete: move |comment_id| {
                                let repo = delete_comment_repo.clone();
                                let flock = delete_comment_flock.clone();
                                let token = token_signal.read().clone();
                                spawn(async move {
                                    let result = api::delete_json::<AdminActionResponse>(
                                        api_base,
                                        token_option(&token),
                                        &format!(
                                            "/repos/{repo}/flocks/{flock}/comments/{comment_id}"
                                        ),
                                    )
                                    .await;
                                    match result {
                                        Ok(_) => {
                                            message.set(t.comment_deleted.to_string());
                                            resource.restart();
                                        }
                                        Err(error) => message.set(error),
                                    }
                                });
                            },
                        }
                    }
                }
            }
        }
        Some(Err(error)) => rsx! { p { class: "error", "{error}" } },
        None => rsx! { p { "{t.loading}" } },
    }
}
