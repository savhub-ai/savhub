use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::{
    BrowseHistoryItem, FlockSummary, PagedResponse, SkillListItem, UserProfileResponse,
};

use crate::api;
use crate::app::{
    Route, SCROLL_PAGE_SIZE, format_local_date, relative_time_i18n, token_option, url_lang,
};
use crate::contexts::{ApiContext, I18nContext};
use crate::i18n::T;
use crate::pages::cards::FlockCard;
use crate::pages::skills_list::SkillListRow;

#[derive(Clone, Copy, PartialEq, Eq)]
enum UserProfilePanel {
    Profile,
    PublishedSkills,
    StarredSkills,
    StarredFlocks,
    RecentHistory,
}

#[component]
pub(crate) fn UserPage(lang: String, handle: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let token = api.token.read().clone();
    let mut active_panel = use_signal(|| UserProfilePanel::Profile);
    {
        let handle = handle.clone();
        use_effect(move || {
            let _ = handle.as_str();
            active_panel.set(UserProfilePanel::Profile);
        });
    }
    let resource = use_resource(move || {
        let token = token.clone();
        let handle = handle.clone();
        async move {
            api::get_json::<UserProfileResponse>(
                api.api_base,
                token_option(&token),
                &format!("/users/{handle}"),
            )
            .await
        }
    });
    match &*resource.read_unchecked() {
        Some(Ok(profile)) => {
            let bio = profile.bio.clone().unwrap_or_else(|| t.no_bio.to_string());
            let display_name = profile
                .user
                .display_name
                .clone()
                .unwrap_or_else(|| format!("@{}", profile.user.handle));
            let joined_at = format_local_date(profile.joined_at);
            let github_login = profile
                .github_login
                .clone()
                .unwrap_or_else(|| profile.user.handle.clone());
            let current_panel = *active_panel.read();
            let published_count = profile.stats.published_skills;
            let starred_skill_count = profile.stats.starred_skills;
            let starred_flock_count = profile.stats.starred_flocks;
            let history_count = profile.stats.history;
            let published_path = format!("/users/{}/published-skills", profile.user.handle);
            let starred_skills_path = format!("/users/{}/starred-skills", profile.user.handle);
            let starred_flocks_path = format!("/users/{}/starred-flocks", profile.user.handle);
            let history_path = format!("/users/{}/history", profile.user.handle);
            rsx! {
                document::Title { "@{profile.user.handle} - {t.brand_name}" }
                section { class: "section",
                    div { class: "profile-shell",
                        aside { class: "profile-sidebar",
                            div { class: "profile-identity-card",
                                div { class: "profile-identity-head",
                                    if let Some(url) = profile.user.avatar_url.clone() {
                                        img { src: "{url}", alt: "@{profile.user.handle}", class: "profile-avatar" }
                                    } else {
                                        div { class: "profile-avatar profile-avatar-placeholder",
                                            "{profile.user.handle.chars().next().unwrap_or('?')}"
                                        }
                                    }
                                    div { class: "profile-identity-copy",
                                        h1 { class: "profile-display-name", "{display_name}" }
                                        p { class: "profile-handle", "@{profile.user.handle}" }
                                    }
                                }
                                div { class: "profile-meta-list",
                                    if !github_login.is_empty() {
                                        div { class: "profile-meta-item",
                                            span { class: "muted", "GitHub" }
                                            span { "@{github_login}" }
                                        }
                                    }
                                }
                                nav { class: "profile-nav",
                                div { class: "profile-nav-list",
                                    button {
                                        class: if current_panel == UserProfilePanel::Profile { "profile-nav-item is-active" } else { "profile-nav-item" },
                                        onclick: move |_| active_panel.set(UserProfilePanel::Profile),
                                        span { class: "profile-nav-label", "{t.profile}" }
                                    }
                                    button {
                                        class: if current_panel == UserProfilePanel::PublishedSkills { "profile-nav-item is-active" } else { "profile-nav-item" },
                                        onclick: move |_| active_panel.set(UserProfilePanel::PublishedSkills),
                                        span { class: "profile-nav-label", "{t.published_skills}" }
                                        span { class: "profile-nav-count", "{published_count}" }
                                    }
                                    if profile.is_self {
                                        button {
                                            class: if current_panel == UserProfilePanel::StarredSkills { "profile-nav-item is-active" } else { "profile-nav-item" },
                                            onclick: move |_| active_panel.set(UserProfilePanel::StarredSkills),
                                            span { class: "profile-nav-label", "{t.starred_skills}" }
                                            span { class: "profile-nav-count", "{starred_skill_count}" }
                                        }
                                        button {
                                            class: if current_panel == UserProfilePanel::StarredFlocks { "profile-nav-item is-active" } else { "profile-nav-item" },
                                            onclick: move |_| active_panel.set(UserProfilePanel::StarredFlocks),
                                            span { class: "profile-nav-label", "{t.starred_flocks}" }
                                            span { class: "profile-nav-count", "{starred_flock_count}" }
                                        }
                                        button {
                                            class: if current_panel == UserProfilePanel::RecentHistory { "profile-nav-item is-active" } else { "profile-nav-item" },
                                            onclick: move |_| active_panel.set(UserProfilePanel::RecentHistory),
                                            span { class: "profile-nav-label", "{t.recent_history}" }
                                            span { class: "profile-nav-count", "{history_count}" }
                                        }
                                    }
                                }
                                }
                            }
                        }
                        div { class: "profile-main",
                            div { class: if current_panel == UserProfilePanel::Profile { "profile-panel-slot" } else { "profile-panel-slot profile-panel-hidden" },
                                div { class: "profile-content-panel",
                                    div { class: "profile-content-body",
                                        div { class: "profile-overview-grid",
                                            div { class: "profile-overview-card",
                                                h3 { "{t.personal_info}" }
                                                div { class: "profile-detail-list",
                                                    div { class: "profile-detail-row",
                                                        span { class: "profile-detail-label", "{t.profile}" }
                                                        div { class: "profile-detail-value",
                                                            strong { "{display_name}" }
                                                            span { class: "muted", "@{profile.user.handle}" }
                                                        }
                                                    }
                                                    div { class: "profile-detail-row",
                                                        span { class: "profile-detail-label", "GitHub" }
                                                        div { class: "profile-detail-value", "@{github_login}" }
                                                    }
                                                    div { class: "profile-detail-row",
                                                        span { class: "profile-detail-label", "{t.role}" }
                                                        div { class: "profile-detail-value",
                                                            span { class: "pill", "{profile.user.role:?}" }
                                                        }
                                                    }
                                                    div { class: "profile-detail-row",
                                                        span { class: "profile-detail-label", "{t.joined_label}" }
                                                        div { class: "profile-detail-value", "{joined_at}" }
                                                    }
                                                    div { class: "profile-detail-row",
                                                        span { class: "profile-detail-label", "Bio" }
                                                        div { class: "profile-detail-value", "{bio}" }
                                                    }
                                                }
                                            }
                                            div { class: "profile-overview-card",
                                                h3 { if profile.is_self { "{t.private_activity}" } else { "{t.published_skills}" } }
                                                div { class: "profile-stat-grid",
                                                    div { class: "profile-stat-tile",
                                                        strong { "{published_count}" }
                                                        span { "{t.published_skills}" }
                                                    }
                                                    if profile.is_self {
                                                        div { class: "profile-stat-tile",
                                                            strong { "{starred_skill_count}" }
                                                            span { "{t.starred_skills}" }
                                                        }
                                                        div { class: "profile-stat-tile",
                                                            strong { "{starred_flock_count}" }
                                                            span { "{t.starred_flocks}" }
                                                        }
                                                        div { class: "profile-stat-tile",
                                                            strong { "{history_count}" }
                                                            span { "{t.recent_history}" }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: if current_panel == UserProfilePanel::PublishedSkills { "profile-panel-slot" } else { "profile-panel-slot profile-panel-hidden" },
                                UserSkillCollectionPanel { active: current_panel == UserProfilePanel::PublishedSkills, handle: profile.user.handle.clone(), path: published_path, title: t.published_skills, description: bio.clone(), empty_label: t.no_published_skills, count: published_count }
                            }
                            if profile.is_self {
                                div { class: if current_panel == UserProfilePanel::StarredSkills { "profile-panel-slot" } else { "profile-panel-slot profile-panel-hidden" },
                                    UserSkillCollectionPanel { active: current_panel == UserProfilePanel::StarredSkills, handle: profile.user.handle.clone(), path: starred_skills_path, title: t.starred_skills, description: t.private_activity_hint.to_string(), empty_label: t.no_starred_skills, count: starred_skill_count }
                                }
                                div { class: if current_panel == UserProfilePanel::StarredFlocks { "profile-panel-slot" } else { "profile-panel-slot profile-panel-hidden" },
                                    UserFlockCollectionPanel { active: current_panel == UserProfilePanel::StarredFlocks, handle: profile.user.handle.clone(), path: starred_flocks_path, title: t.starred_flocks, description: t.private_activity_hint.to_string(), empty_label: t.no_starred_flocks, count: starred_flock_count }
                                }
                                div { class: if current_panel == UserProfilePanel::RecentHistory { "profile-panel-slot" } else { "profile-panel-slot profile-panel-hidden" },
                                    UserHistoryCollectionPanel { active: current_panel == UserProfilePanel::RecentHistory, handle: profile.user.handle.clone(), path: history_path, title: t.recent_history, description: t.private_activity_hint.to_string(), empty_label: t.no_history_yet, count: history_count }
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(Err(error)) => rsx! { p { class: "error", "{error}" } },
        None => rsx! { p { "{t.loading}" } },
    }
}

#[component]
fn UserSkillCollectionPanel(
    active: bool,
    handle: String,
    path: String,
    title: &'static str,
    description: String,
    empty_label: &'static str,
    count: i64,
) -> Element {
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let token = api.token.read().clone();
    let mut items = use_signal(Vec::<SkillListItem>::new);
    let mut next_cursor = use_signal(|| None::<String>);
    let mut loaded = use_signal(|| false);
    let mut loading = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let fetch_token = token.clone();
    let fetch_path = path.clone();
    let fetch_page = move |cursor: Option<String>, append: bool| {
        let token = fetch_token.clone();
        let path = fetch_path.clone();
        spawn(async move {
            loading.set(true);
            let mut request_path = format!("{path}?limit={SCROLL_PAGE_SIZE}");
            if let Some(cursor) = cursor.as_deref() {
                request_path.push_str(&format!("&cursor={cursor}"));
            }
            match api::get_json::<PagedResponse<SkillListItem>>(
                api.api_base,
                token_option(&token),
                &request_path,
            )
            .await
            {
                Ok(payload) => {
                    if append {
                        items.write().extend(payload.items);
                    } else {
                        items.set(payload.items);
                    }
                    next_cursor.set(payload.next_cursor);
                    loaded.set(true);
                    error.set(None);
                }
                Err(message) => error.set(Some(message)),
            }
            loading.set(false);
        });
    };

    if active && !*loaded.read() && !*loading.read() {
        fetch_page(None, false);
    }

    let next_value = next_cursor.read().clone();
    let is_loading = *loading.read();
    let items_value = items.read().clone();
    let error_value = error.read().clone();

    rsx! {
        div { class: "profile-content-panel profile-collection",
            div { class: "profile-content-header",
                div {
                    p { class: "profile-content-kicker", "@{handle}" }
                    h2 { class: "profile-content-title", "{title}" }
                    p { class: "profile-content-desc", "{description}" }
                }
                span { class: "profile-content-count", "{count}" }
            }
            div { class: "profile-content-body",
                if let Some(message) = error_value {
                    p { class: "error", "{message}" }
                } else if !*loaded.read() && is_loading {
                    p { class: "scroll-loader", "{t.loading}" }
                } else if items_value.is_empty() {
                    div { class: "empty-state profile-empty-state", "{empty_label}" }
                } else {
                    div { class: "list-view",
                        for skill in items_value {
                            SkillListRow { skill }
                        }
                    }
                    if let Some(cursor) = next_value {
                        div { class: "profile-panel-footer",
                            button {
                                class: "btn-sm",
                                disabled: is_loading,
                                onclick: move |_| fetch_page(Some(cursor.clone()), true),
                                if is_loading { "{t.loading}" } else { "{t.load_more}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn UserFlockCollectionPanel(
    active: bool,
    handle: String,
    path: String,
    title: &'static str,
    description: String,
    empty_label: &'static str,
    count: i64,
) -> Element {
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let token = api.token.read().clone();
    let mut items = use_signal(Vec::<FlockSummary>::new);
    let mut next_cursor = use_signal(|| None::<String>);
    let mut loaded = use_signal(|| false);
    let mut loading = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let fetch_token = token.clone();
    let fetch_path = path.clone();
    let fetch_page = move |cursor: Option<String>, append: bool| {
        let token = fetch_token.clone();
        let path = fetch_path.clone();
        spawn(async move {
            loading.set(true);
            let mut request_path = format!("{path}?limit={SCROLL_PAGE_SIZE}");
            if let Some(cursor) = cursor.as_deref() {
                request_path.push_str(&format!("&cursor={cursor}"));
            }
            match api::get_json::<PagedResponse<FlockSummary>>(
                api.api_base,
                token_option(&token),
                &request_path,
            )
            .await
            {
                Ok(payload) => {
                    if append {
                        items.write().extend(payload.items);
                    } else {
                        items.set(payload.items);
                    }
                    next_cursor.set(payload.next_cursor);
                    loaded.set(true);
                    error.set(None);
                }
                Err(message) => error.set(Some(message)),
            }
            loading.set(false);
        });
    };

    if active && !*loaded.read() && !*loading.read() {
        fetch_page(None, false);
    }

    let next_value = next_cursor.read().clone();
    let is_loading = *loading.read();
    let items_value = items.read().clone();
    let error_value = error.read().clone();

    rsx! {
        div { class: "profile-content-panel profile-collection",
            div { class: "profile-content-header",
                div {
                    p { class: "profile-content-kicker", "@{handle}" }
                    h2 { class: "profile-content-title", "{title}" }
                    p { class: "profile-content-desc", "{description}" }
                }
                span { class: "profile-content-count", "{count}" }
            }
            div { class: "profile-content-body",
                if let Some(message) = error_value {
                    p { class: "error", "{message}" }
                } else if !*loaded.read() && is_loading {
                    p { class: "scroll-loader", "{t.loading}" }
                } else if items_value.is_empty() {
                    div { class: "empty-state profile-empty-state", "{empty_label}" }
                } else {
                    div { class: "card-grid",
                        for flock in items_value {
                            FlockCard { flock }
                        }
                    }
                    if let Some(cursor) = next_value {
                        div { class: "profile-panel-footer",
                            button {
                                class: "btn-sm",
                                disabled: is_loading,
                                onclick: move |_| fetch_page(Some(cursor.clone()), true),
                                if is_loading { "{t.loading}" } else { "{t.load_more}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn UserHistoryCollectionPanel(
    active: bool,
    handle: String,
    path: String,
    title: &'static str,
    description: String,
    empty_label: &'static str,
    count: i64,
) -> Element {
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let token = api.token.read().clone();
    let mut items = use_signal(Vec::<BrowseHistoryItem>::new);
    let mut next_cursor = use_signal(|| None::<String>);
    let mut loaded = use_signal(|| false);
    let mut loading = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let fetch_token = token.clone();
    let fetch_path = path.clone();
    let fetch_page = move |cursor: Option<String>, append: bool| {
        let token = fetch_token.clone();
        let path = fetch_path.clone();
        spawn(async move {
            loading.set(true);
            let mut request_path = format!("{path}?limit={SCROLL_PAGE_SIZE}");
            if let Some(cursor) = cursor.as_deref() {
                request_path.push_str(&format!("&cursor={cursor}"));
            }
            match api::get_json::<PagedResponse<BrowseHistoryItem>>(
                api.api_base,
                token_option(&token),
                &request_path,
            )
            .await
            {
                Ok(payload) => {
                    if append {
                        items.write().extend(payload.items);
                    } else {
                        items.set(payload.items);
                    }
                    next_cursor.set(payload.next_cursor);
                    loaded.set(true);
                    error.set(None);
                }
                Err(message) => error.set(Some(message)),
            }
            loading.set(false);
        });
    };

    if active && !*loaded.read() && !*loading.read() {
        fetch_page(None, false);
    }

    let next_value = next_cursor.read().clone();
    let is_loading = *loading.read();
    let items_value = items.read().clone();
    let error_value = error.read().clone();

    rsx! {
        div { class: "profile-content-panel profile-collection",
            div { class: "profile-content-header",
                div {
                    p { class: "profile-content-kicker", "@{handle}" }
                    h2 { class: "profile-content-title", "{title}" }
                    p { class: "profile-content-desc", "{description}" }
                }
                span { class: "profile-content-count", "{count}" }
            }
            div { class: "profile-content-body",
                if let Some(message) = error_value {
                    p { class: "error", "{message}" }
                } else if !*loaded.read() && is_loading {
                    p { class: "scroll-loader", "{t.loading}" }
                } else if items_value.is_empty() {
                    div { class: "empty-state profile-empty-state", "{empty_label}" }
                } else {
                    HistoryList { items: items_value }
                    if let Some(cursor) = next_value {
                        div { class: "profile-panel-footer",
                            button {
                                class: "btn-sm",
                                disabled: is_loading,
                                onclick: move |_| fetch_page(Some(cursor.clone()), true),
                                if is_loading { "{t.loading}" } else { "{t.load_more}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn HistoryList(items: Vec<BrowseHistoryItem>) -> Element {
    let t = use_context::<I18nContext>().t();
    rsx! {
        ul { class: "profile-history-list",
            for item in items {
                {
                    let label = history_kind_label(&item, t);
                    let viewed = relative_time_i18n(item.viewed_at, t);
                    let owner_handle = item.owner_handle.clone();
                    let title = item.resource_title.clone();
                    match history_route(&item) {
                        Some(route) => rsx! {
                            li { class: "profile-history-item",
                                div { class: "profile-history-main",
                                    Link { to: route, class: "profile-history-link", "{title}" }
                                    if let Some(owner) = owner_handle {
                                        span { class: "muted", "@{owner}" }
                                    }
                                }
                                div { class: "profile-history-meta",
                                    span { class: "pill", "{label}" }
                                    span { class: "relative-time", "{viewed}" }
                                }
                            }
                        },
                        None => rsx! {
                            li { class: "profile-history-item",
                                div { class: "profile-history-main",
                                    span { class: "profile-history-link", "{title}" }
                                    if let Some(owner) = owner_handle {
                                        span { class: "muted", "@{owner}" }
                                    }
                                }
                                div { class: "profile-history-meta",
                                    span { class: "pill", "{label}" }
                                    span { class: "relative-time", "{viewed}" }
                                }
                            }
                        },
                    }
                }
            }
        }
    }
}

fn history_route(item: &BrowseHistoryItem) -> Option<Route> {
    let lang = url_lang();
    match item.resource_type.as_str() {
        "skill" => Some(Route::SkillPage {
            lang,
            id: item.resource_id.to_string(),
        }),
        "repo" => {
            let parts: Vec<&str> = item.resource_slug.splitn(3, '/').collect();
            Some(Route::RepoPage {
                lang,
                domain: parts.first().unwrap_or(&"").to_string(),
                owner: parts.get(1).unwrap_or(&"").to_string(),
                name: parts.get(2).unwrap_or(&"").to_string(),
            })
        }
        "flock" => Some(Route::FlockPage {
            lang,
            id: item.resource_id.to_string(),
        }),
        _ => None,
    }
}

fn history_kind_label(item: &BrowseHistoryItem, t: &T) -> String {
    match item.resource_type.as_str() {
        "skill" => t.nav_skills.to_string(),
        "repo" => t.nav_repos.to_string(),
        "flock" => t.flocks.to_string(),
        _ => item.resource_type.clone(),
    }
}
