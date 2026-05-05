use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::{
    RecordViewRequest, RepoDetailResponse, SubmitIndexRequest, SubmitIndexResponse,
};

use crate::api;
use crate::app::{Route, friendly_error, token_option, url_lang};
use crate::contexts::{ApiContext, I18nContext, ToastContext};
use crate::pages::skills_list::SkillFlockList;

#[component]
pub(crate) fn RepoPage(lang: String, domain: String, owner: String, name: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let toast = use_context::<ToastContext>();
    let token = api.token.read().clone();
    let mut reindex_loading = use_signal(|| false);
    let slug = format!("{domain}/{owner}/{name}");
    let mut resource = use_resource(move || {
        let token = token.clone();
        let slug = slug.clone();
        async move {
            api::get_json::<RepoDetailResponse>(
                api.api_base,
                token_option(&token),
                &format!("/repos/{slug}"),
            )
            .await
        }
    });
    match &*resource.read_unchecked() {
        Some(Ok(payload)) => {
            let links: Vec<(String, String)> = Vec::new();
            let maintainers = payload
                .document
                .metadata
                .maintainers
                .iter()
                .map(|maintainer| {
                    (
                        maintainer.name.clone(),
                        maintainer
                            .role
                            .clone()
                            .unwrap_or_else(|| "maintainer".to_string()),
                    )
                })
                .collect::<Vec<_>>();
            // Record browse history
            {
                let repo_id = payload.repo.id;
                let view_token = api.token.read().clone();
                let api_base = api.api_base;
                spawn(async move {
                    if !view_token.trim().is_empty() {
                        let _ = api::post_json::<_, serde_json::Value>(
                            api_base,
                            token_option(&view_token),
                            "/history",
                            &RecordViewRequest {
                                resource_type: "repo".to_string(),
                                resource_id: repo_id,
                            },
                        )
                        .await;
                    }
                });
            }
            rsx! {
                document::Title { "{payload.repo.name} - {t.brand_name}" }
                section { class: "section detail",
                    nav { class: "breadcrumb",
                        Link { to: Route::ReposPage { lang: url_lang() }, "{t.nav_repos}" }
                        span { class: "breadcrumb-sep", "/" }
                        span { "{payload.repo.name}" }
                    }
                    div { class: "detail-head",
                        div {
                            h1 { "{payload.repo.name}" }
                            a { class: "muted", href: "{payload.repo.git_url}", target: "_blank", rel: "noreferrer", "{payload.repo.git_url}" }
                        }
                        {
                            let token_val = api.token.read().clone();
                            let is_logged_in = !token_val.trim().is_empty();
                            if is_logged_in {
                                let git_url = payload.repo.git_url.clone();
                                let git_branch = payload.repo.git_branch.clone().unwrap_or_else(|| "main".to_string());
                                rsx! {
                                    div { class: "detail-actions",
                                        if *reindex_loading.read() {
                                            span { class: "btn-sm",
                                                span { class: "spinner" }
                                                "{t.reindex}"
                                            }
                                        } else {
                                            button {
                                                class: "btn-sm",
                                                onclick: move |_| {
                                                    if *reindex_loading.read() { return; }
                                                    reindex_loading.set(true);
                                                    let token = api.token.read().clone();
                                                    let url = git_url.clone();
                                                    let branch = git_branch.clone();
                                                    spawn(async move {
                                                        let body = SubmitIndexRequest {
                                                            git_url: url,
                                                            git_ref: branch,
                                                            git_subdir: ".".to_string(),
                                                            repo_slug: None,
                                                            force: true,
                                                        };
                                                        match api::post_json::<_, SubmitIndexResponse>(
                                                            api.api_base,
                                                            token_option(&token),
                                                            "/index",
                                                            &body,
                                                        ).await {
                                                            Ok(_) => {
                                                                toast.success(t.reindex_submitted.to_string());
                                                                resource.restart();
                                                            }
                                                            Err(e) => {
                                                                toast.error(friendly_error(&e, t));
                                                            }
                                                        }
                                                        reindex_loading.set(false);
                                                    });
                                                },
                                                "{t.reindex}"
                                            }
                                        }
                                    }
                                }
                            } else {
                                rsx! {}
                            }
                        }
                    }
                    p { class: "summary detail-summary", "{payload.repo.description}" }
                    div { class: "meta-row wide",
                        span { "{payload.repo.skill_count} {t.nav_skills}" }
                        span { "{payload.repo.visibility:?}" }
                        span { if payload.repo.verified { "{t.verified}" } else { "{t.unverified}" } }
                    }
                    if !maintainers.is_empty() || !links.is_empty() {
                        div { class: "detail-grid",
                            if !maintainers.is_empty() {
                                div { class: "panel",
                                    h2 { "{t.maintainers}" }
                                    ul { class: "dense-list",
                                        for (name, role) in maintainers {
                                            li { "{name} ({role})" }
                                        }
                                    }
                                }
                            }
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
                    }
                    SkillFlockList {
                        title: t.indexed_skills.to_string(),
                        storage_prefix: format!("savhub.repo.{}", payload.repo.id),
                        repo_id: Some(payload.repo.id.to_string()),
                        repo_name: Some(payload.repo.name.clone()),
                        sticky: true,
                    }
                }
            }
        }
        Some(Err(error)) => rsx! { p { class: "error", "{error}" } },
        None => rsx! { p { "{t.loading}" } },
    }
}
