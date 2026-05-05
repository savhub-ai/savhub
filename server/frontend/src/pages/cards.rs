//! Card components for browsing skills, flocks, and repos in grids.

use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::SkillListItem;

use crate::app::{
    Route, render_copy_icon, render_security_badge, render_source_label, repo_route, url_lang,
};
use crate::contexts::I18nContext;
use crate::urls::derive_repo_slug;

#[component]
pub(crate) fn SkillCard(skill: SkillListItem) -> Element {
    let t = use_context::<I18nContext>().t();
    let latest = skill
        .latest_version
        .as_ref()
        .map(|v| format!("v{}", v.version))
        .unwrap_or_else(|| "-".to_string());
    let summary = skill
        .summary
        .clone()
        .unwrap_or_else(|| t.no_summary.to_string());
    let badges = [
        skill.badges.highlighted.then_some("highlighted"),
        skill.badges.official.then_some("official"),
        skill.badges.deprecated.then_some("deprecated"),
        skill.badges.suspicious.then_some("suspicious"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    rsx! {
        article { class: "card",
            div { class: "card-head",
                div {
                    h3 { class: "card-title-row",
                        Link { to: Route::SkillPage { lang: url_lang(), id: skill.id.to_string() }, "{skill.display_name}" }
                        { render_security_badge(&skill.security_status, t) }
                        {
                            render_copy_icon(&skill.repo_url, &skill.path)
                        }
                    }
                }
                span { class: "pill", "{latest}" }
            }
            p { class: "summary", "{summary}" }
            div { class: "meta-row",
                span { "{skill.stats.downloads} {t.downloads}" }
                span { "{skill.stats.stars} {t.stars}" }
                span { "{skill.stats.comments} {t.comments}" }
            }
            if !badges.is_empty() {
                div { class: "badge-row",
                    for badge in badges {
                        span { class: "badge", "{badge}" }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn RepoCard(repo: savhub_shared::RepoSummary) -> Element {
    let t = use_context::<I18nContext>().t();
    let slug = derive_repo_slug(&repo.git_url);
    rsx! {
        article { class: "card",
            div { class: "card-head",
                div {
                    h3 { Link { to: repo_route(&slug), "{repo.name}" } }
                    p { class: "muted", "{slug}" }
                }
                span { class: "pill", "{repo.flock_count} repos" }
            }
            p { class: "summary", "{repo.description}" }
            div { class: "meta-row",
                span { "{repo.visibility:?}" }
                span { if repo.verified { "{t.verified}" } else { "{t.community}" } }
            }
        }
    }
}

#[component]
pub(crate) fn FlockCard(flock: savhub_shared::FlockSummary) -> Element {
    let t = use_context::<I18nContext>().t();
    rsx! {
        article { class: "card",
            div { class: "card-head",
                div {
                    h3 { class: "card-title-row",
                        Link {
                            to: Route::FlockPage {
                                lang: url_lang(),
                                id: flock.id.to_string(),
                            },
                            "{flock.name}"
                        }
                        { render_security_badge(&flock.security_status, t) }
                        { render_copy_icon(&flock.repo_url, &flock.slug) }
                    }
                }
                span { class: "pill", "{flock.skill_count} {t.nav_skills}" }
            }
            p { class: "summary", "{flock.description}" }
            div { class: "meta-row",
                if let Some(ref v) = flock.version {
                    span { "v{v}" }
                }
                span { "{flock.status:?}" }
                span { "{render_source_label(&flock.source, t)}" }
            }
        }
    }
}
