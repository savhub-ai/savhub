use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::{FlockSummary, PagedResponse, SkillListItem};

use crate::api;
use crate::app::{
    Route, SCROLL_PAGE_SIZE, relative_time_i18n, render_copy_sign, render_security_badge,
    token_option, url_lang,
};
use crate::contexts::{ApiContext, I18nContext};
use crate::location::{near_window_bottom, query_param, set_location_query};
use crate::storage::{load_storage, save_storage};

#[component]
pub(crate) fn SkillsPage(lang: String) -> Element {
    let _ = &lang;
    let t = use_context::<I18nContext>().t();
    rsx! {
        document::Title { "{t.skills_title}" }
        section { class: "section",
            SkillFlockList {
                title: t.skills_heading.to_string(),
                storage_prefix: "savhub.skills".to_string(),
                sync_url: true,
            }
        }
    }
}

/// Reusable skill/flock list with search, sort, grouped toggle, infinite scroll.
/// When `repo_id` is provided, results are scoped to that repository.
#[component]
pub(crate) fn SkillFlockList(
    title: String,
    storage_prefix: String,
    #[props(default = false)] sync_url: bool,
    #[props(default)] repo_id: Option<String>,
    #[props(default)] flock_id: Option<String>,
    #[props(default)] repo_name: Option<String>,
    #[props(default = false)] sticky: bool,
    #[props(default = false)] hide_grouped: bool,
) -> Element {
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();

    let storage_key = storage_prefix.clone();
    let init_q = if sync_url {
        query_param("q")
            .or_else(|| load_storage(&format!("{storage_key}.q")))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let init_sort = if sync_url {
        query_param("sort")
            .filter(|v| {
                matches!(
                    v.as_str(),
                    "stars" | "updated" | "name" | "installs" | "users"
                )
            })
            .or_else(|| {
                load_storage(&format!("{storage_key}.sort")).filter(|v| {
                    matches!(
                        v.as_str(),
                        "stars" | "updated" | "name" | "installs" | "users"
                    )
                })
            })
            .unwrap_or_else(|| "stars".to_string())
    } else {
        "stars".to_string()
    };

    let init_q2 = init_q.clone();
    let mut search_input = use_signal(move || init_q.clone());
    let mut search_query = use_signal(move || init_q2.clone());
    let mut debounce_gen = use_signal(|| 0u32);
    let mut sort = use_signal(move || init_sort.clone());
    let no_grouped = hide_grouped;
    let mut grouped = use_signal(move || !no_grouped);

    // Infinite scroll state
    let mut skill_items = use_signal(Vec::<SkillListItem>::new);
    let mut flock_items = use_signal(Vec::<FlockSummary>::new);
    let mut has_more = use_signal(|| true);
    let mut loading = use_signal(|| false);
    let mut reset_gen = use_signal(|| 0u32);
    let repo_filter = use_signal(move || repo_id);
    let flock_filter = use_signal(move || flock_id);

    // URL sync (simplified — no cursor/limit since infinite scroll)
    let do_sync_url = sync_url;
    let storage_key2 = storage_prefix.clone();
    use_effect(move || {
        if !do_sync_url {
            return;
        }
        let s = sort.read().clone();
        let q = search_query.read().clone();
        save_storage(&format!("{storage_key2}.sort"), &s);
        save_storage(&format!("{storage_key2}.q"), &q);
        set_location_query(&[("sort", &s), ("q", &q)]);
    });

    // Reset items when search/sort/grouped changes
    use_effect(move || {
        let _ = search_query.read();
        let _ = sort.read();
        let _ = grouped.read();
        skill_items.set(Vec::new());
        flock_items.set(Vec::new());
        has_more.set(true);
        {
            let v = *reset_gen.peek() + 1;
            reset_gen.set(v);
        }
    });

    // Fetch closure — all captures are Copy
    let fetch_items = move |append: bool| {
        let token = api.token.peek().clone();
        let search_val = search_query.peek().clone();
        let sort_val = sort.peek().clone();
        let is_grouped = *grouped.peek();
        let repo_param = repo_filter.peek().clone();
        let flock_param = flock_filter.peek().clone();
        let cur_gen = *reset_gen.peek();
        spawn(async move {
            if *loading.peek() {
                return;
            }
            loading.set(true);
            let offset = if append {
                if is_grouped {
                    flock_items.peek().len()
                } else {
                    skill_items.peek().len()
                }
            } else {
                0
            } as i64;

            if is_grouped {
                let mut url =
                    format!("/flocks?limit={SCROLL_PAGE_SIZE}&sort={sort_val}&cursor={offset}");
                if !search_val.trim().is_empty() {
                    url.push_str(&format!("&q={}", search_val.trim()));
                }
                if let Some(ref rid) = repo_param {
                    url.push_str(&format!("&repo={rid}"));
                }
                match api::get_json::<PagedResponse<FlockSummary>>(
                    api.api_base,
                    token_option(&token),
                    &url,
                )
                .await
                {
                    Ok(data) => {
                        if *reset_gen.peek() != cur_gen {
                            loading.set(false);
                            return;
                        }
                        has_more.set(data.next_cursor.is_some());
                        if append {
                            flock_items.write().extend(data.items);
                        } else {
                            flock_items.set(data.items);
                        }
                    }
                    Err(_) => has_more.set(false),
                }
            } else {
                let mut url =
                    format!("/skills?limit={SCROLL_PAGE_SIZE}&sort={sort_val}&cursor={offset}");
                if !search_val.trim().is_empty() {
                    url.push_str(&format!("&q={}", search_val.trim()));
                }
                if let Some(ref rid) = repo_param {
                    url.push_str(&format!("&repo={rid}"));
                }
                if let Some(ref fid) = flock_param {
                    url.push_str(&format!("&flock={fid}"));
                }
                match api::get_json::<PagedResponse<SkillListItem>>(
                    api.api_base,
                    token_option(&token),
                    &url,
                )
                .await
                {
                    Ok(data) => {
                        if *reset_gen.peek() != cur_gen {
                            loading.set(false);
                            return;
                        }
                        has_more.set(data.next_cursor.is_some());
                        if append {
                            skill_items.write().extend(data.items);
                        } else {
                            skill_items.set(data.items);
                        }
                    }
                    Err(_) => has_more.set(false),
                }
            }
            loading.set(false);
        });
    };

    // Initial load + reload on reset
    use_effect(move || {
        let _ = reset_gen.read();
        fetch_items(false);
    });

    // Scroll detection — poll window scroll position
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(300).await;
            if !*has_more.peek() || *loading.peek() {
                continue;
            }
            let near_bottom = near_window_bottom(200.0);
            if near_bottom {
                fetch_items(true);
            }
        }
    });

    let is_sticky = sticky;
    let sticky_name = repo_name.clone().unwrap_or_default();
    if is_sticky {
        use_effect(|| {
            document::eval(
                r#"
                (function() {
                    var sentinel = document.getElementById('skills-sticky-sentinel');
                    var header = document.getElementById('skills-sticky-header');
                    if (!sentinel || !header) return;
                    var observer = new IntersectionObserver(function(entries) {
                        if (!entries[0].isIntersecting) {
                            header.classList.add('is-stuck');
                        } else {
                            header.classList.remove('is-stuck');
                        }
                    }, { threshold: 0 });
                    observer.observe(sentinel);
                })();
            "#,
            );
        });
    }

    rsx! {
        if is_sticky {
            div { class: "sticky-sentinel", id: "skills-sticky-sentinel" }
        }
        div {
            class: if is_sticky { "list-toolbar sticky-skills-head" } else { "list-toolbar" },
            id: if is_sticky { "skills-sticky-header" } else { "" },
            h2 { class: "toolbar-title",
                if is_sticky {
                    span { class: "sticky-repo-name", "{sticky_name} " }
                }
                "{title}"
            }
            input {
                class: "search-input",
                r#type: "search",
                placeholder: "{t.search_skills}",
                value: "{search_input}",
                oninput: move |event| {
                    let val = event.value();
                    search_input.set(val.clone());
                    let generation = debounce_gen() + 1;
                    debounce_gen.set(generation);
                    spawn(async move {
                        gloo_timers::future::TimeoutFuture::new(300).await;
                        if debounce_gen() == generation {
                            search_query.set(val);
                        }
                    });
                }
            }
            select {
                value: "{sort}",
                onchange: move |event| {
                    sort.set(event.value());
                },
                option { value: "stars", "{t.sort_stars}" }
                option { value: "updated", "{t.sort_updated}" }
                option { value: "name", "{t.sort_name}" }
                option { value: "installs", "{t.sort_installs}" }
                option { value: "users", "{t.sort_users}" }
            }
            if !no_grouped {
                button {
                    class: if *grouped.read() { "toggle-btn active" } else { "toggle-btn" },
                    onclick: move |_| {
                        let current = *grouped.read();
                        grouped.set(!current);
                    },
                    "{t.grouped_label}"
                }
            }
        }
        if *grouped.read() {
            {
                let items = flock_items.read();
                if items.is_empty() && !*loading.read() {
                    rsx! {
                        div { class: "empty-state",
                            if search_query.read().is_empty() {
                                "{t.no_flocks_yet}"
                            } else {
                                "{t.no_flocks_match}"
                            }
                        }
                    }
                } else {
                    rsx! {
                        div { class: "list-view",
                            for flock in items.iter().cloned() {
                                FlockListRow { flock }
                            }
                        }
                        if *loading.read() {
                            p { class: "scroll-loader", "{t.loading}" }
                        }
                    }
                }
            }
        } else {
            {
                let items = skill_items.read();
                if items.is_empty() && !*loading.read() {
                    rsx! {
                        div { class: "empty-state",
                            if search_query.read().is_empty() {
                                "{t.no_skills_yet}"
                            } else {
                                "{t.no_skills_match}"
                            }
                        }
                    }
                } else {
                    rsx! {
                        div { class: "list-view",
                            for skill in items.iter().cloned() {
                                SkillListRow { skill }
                            }
                        }
                        if *loading.read() {
                            p { class: "scroll-loader", "{t.loading}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn FlockListRow(flock: FlockSummary) -> Element {
    let t = use_context::<I18nContext>().t();
    let updated = relative_time_i18n(flock.updated_at, t);

    rsx! {
        div { class: "list-item",
            div { class: "list-item-main",
                h3 { class: "list-item-title",
                    Link { to: Route::FlockPage { lang: url_lang(), id: flock.id.to_string() }, "{flock.name}" }
                    { render_security_badge(&flock.security_status, t) }
                }
                { render_copy_sign(&flock.repo_url, &flock.slug) }
                if !flock.description.is_empty() {
                    p { class: "list-item-summary", "{flock.description}" }
                }
            }
            div { class: "list-item-meta",
                if let Some(ref v) = flock.version {
                    span { class: "badge", "v{v}" }
                }
                span { class: "meta-text", "{flock.skill_count} {t.flock_skills_count}" }
                if flock.stats_stars > 0 {
                    span { class: "meta-text", style: "display: inline-flex; align-items: center; gap: 3px;",
                        crate::icons::IconStar { size: 12, filled: true }
                        "{flock.stats_stars}"
                    }
                }
                span { class: "relative-time", "{updated}" }
            }
        }
    }
}

#[component]
pub(crate) fn SkillListRow(skill: SkillListItem) -> Element {
    let t = use_context::<I18nContext>().t();
    let summary = skill.summary.clone().unwrap_or_default();
    let latest = skill
        .latest_version
        .as_ref()
        .map(|v| format!("v{}", v.version))
        .unwrap_or_else(|| "-".to_string());
    let updated = relative_time_i18n(skill.updated_at, t);
    let badges: Vec<(&str, &str)> = [
        skill
            .badges
            .highlighted
            .then_some(("highlighted", "badge badge-highlighted")),
        skill
            .badges
            .official
            .then_some(("official", "badge badge-official")),
        skill
            .badges
            .deprecated
            .then_some(("deprecated", "badge badge-deprecated")),
        skill
            .badges
            .suspicious
            .then_some(("suspicious", "badge badge-suspicious")),
    ]
    .into_iter()
    .flatten()
    .collect();
    rsx! {
        div { class: "list-item",
            div { class: "list-item-main",
                h3 {
                    Link { to: Route::SkillPage { lang: url_lang(), id: skill.id.to_string() }, "{skill.display_name}" }
                    { render_security_badge(&skill.security_status, t) }
                }
                { render_copy_sign(&skill.repo_url, &skill.path) }
                if !summary.is_empty() {
                    p { class: "list-item-summary", "{summary}" }
                }
                if !badges.is_empty() {
                    div { class: "list-item-meta",
                        for (label, class) in badges {
                            span { class: "{class}", "{label}" }
                        }
                    }
                }
            }
            div { class: "list-item-right",
                span { class: "relative-time", "{updated}" }
                div { class: "list-item-stats",
                    if latest != "-" {
                        span { "{latest}" }
                    }
                    span { "{skill.stats.downloads} {t.dl}" }
                    span { "{skill.stats.stars} {t.stars}" }
                    span { "{skill.stats.comments} {t.comments}" }
                }
            }
        }
    }
}
