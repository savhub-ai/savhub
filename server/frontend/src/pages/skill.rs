use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::{
    AdminActionResponse, CreateCommentRequest, RecordViewRequest, SkillDetailResponse,
    ToggleStarResponse,
};

use crate::api;
use crate::app::{
    CommentsList, MetadataPanel, Route, format_local_datetime, render_copy_sign,
    render_security_badge, token_option, url_lang,
};
use crate::contexts::{ApiContext, I18nContext};
use crate::pages::widgets::CommentComposer;

#[component]
pub(crate) fn SkillPage(lang: String, id: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let mut message = use_signal(String::new);
    let mut comment_body = use_signal(String::new);

    let token = api.token.read().clone();
    let id_for_resource = id.clone();
    let detail = use_resource(move || {
        let token = token.clone();
        let id_for_resource = id_for_resource.clone();
        async move {
            api::get_json::<SkillDetailResponse>(
                api.api_base,
                token_option(&token),
                &format!("/skills/{id_for_resource}"),
            )
            .await
        }
    });

    let comment_value = comment_body.read().clone();

    let content = match &*detail.read_unchecked() {
        Some(Ok(payload)) => {
            let latest = payload.latest_version.clone();
            let latest_metadata = latest
                .as_ref()
                .and_then(|value| value.bundle_metadata.clone());
            let comments = payload.comments.clone();
            let versions = payload.versions.clone();
            let skill_id_str = payload.skill.id.to_string();
            let star_id = skill_id_str.clone();
            let comment_id_str = skill_id_str.clone();
            let delete_comment_id_str = skill_id_str.clone();
            let api_base = api.api_base;
            let token_signal = api.token;
            let mut resource = detail;
            // Record browse history
            {
                let skill_id = payload.skill.id;
                let view_token = api.token.read().clone();
                spawn(async move {
                    if !view_token.trim().is_empty() {
                        let _ = api::post_json::<_, serde_json::Value>(
                            api_base,
                            token_option(&view_token),
                            "/history",
                            &RecordViewRequest {
                                resource_type: "skill".to_string(),
                                resource_id: skill_id,
                            },
                        )
                        .await;
                    }
                });
            }
            rsx! {
                document::Title { "{payload.skill.display_name} - {t.brand_name}" }
                section { class: "section detail",
                    nav { class: "breadcrumb",
                        Link { to: Route::SkillsPage { lang: url_lang() }, "{t.nav_skills}" }
                        span { class: "breadcrumb-sep", "/" }
                        span { "{payload.skill.display_name}" }
                    }
                    div { class: "detail-head",
                        div {
                            h1 { "{payload.skill.display_name}"
                                { render_security_badge(&payload.skill.security_status, t) }
                            }
                            { render_copy_sign(&payload.skill.repo_url, &payload.skill.path) }
                        }
                        div { class: "detail-actions",
                            button {
                                class: "primary",
                                onclick: move |_| {
                                    let sid = star_id.clone();
                                    let token = token_signal.read().clone();
                                    if token.trim().is_empty() {
                                        message.set(t.login_before_star.to_string());
                                        return;
                                    }
                                    spawn(async move {
                                        let result = api::post_empty::<ToggleStarResponse>(
                                            api_base,
                                            token_option(&token),
                                            &format!("/skills/{sid}/star"),
                                        )
                                        .await;
                                        match result {
                                            Ok(payload) => message.set(format!("Skill star state updated: {} stars.", payload.stars)),
                                            Err(error) => message.set(error),
                                        }
                                        resource.restart();
                                    });
                                },
                                if payload.starred { "{t.unstar}" } else { "{t.star}" }
                            }
                        }
                    }
                    div { class: "meta-row wide",
                        span { "{payload.skill.stats.downloads} {t.downloads}" }
                        span { "{payload.skill.stats.stars} {t.stars}" }
                        span { "{payload.skill.stats.comments} {t.comments}" }
                        span { "{payload.skill.stats.versions} {t.versions}" }
                        span { "{payload.skill.moderation_status:?}" }
                    }
                    p { class: "summary detail-summary", "{payload.skill.summary.clone().unwrap_or_default()}" }
                    if let Some(latest) = latest {
                        article { class: "markdown-panel", dangerous_inner_html: "{latest.markdown_html}" }
                        section { class: "detail-grid",
                            if let Some(metadata) = latest_metadata.clone() {
                                MetadataPanel { metadata }
                            }
                            div { class: "panel",
                                h2 { "{t.files}" }
                                ul { class: "dense-list",
                                    for file in latest.files {
                                        li { code { "{file.path}" } " · {file.size} bytes" }
                                    }
                                }
                            }
                            div { class: "panel",
                                h2 { "{t.versions}" }
                                ul { class: "dense-list",
                                    for version in versions {
                                        { let ts = format_local_datetime(version.created_at); rsx! { li { "v{version.version} · {ts}" } } }
                                    }
                                }
                            }
                        }
                    }
                    section { class: "panel",
                        h2 { "{t.comments}" }
                        CommentComposer {
                            body: comment_value,
                            placeholder: t.comment_placeholder,
                            on_input: move |value| comment_body.set(value),
                            on_submit: move |_| {
                                let sid = comment_id_str.clone();
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
                                        &format!("/skills/{sid}/comments"),
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
                                let sid = delete_comment_id_str.clone();
                                let token = token_signal.read().clone();
                                spawn(async move {
                                    let result = api::delete_json::<AdminActionResponse>(
                                        api_base,
                                        token_option(&token),
                                        &format!("/skills/{sid}/comments/{comment_id}"),
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
    };

    let notice = message.read().clone();
    rsx! {
        if !notice.is_empty() {
            p { class: "notice", "{notice}" }
        }
        {content}
    }
}
