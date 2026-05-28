use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::{
    PagedResponse, RepoSummary, SubmitIndexRequest, SubmitIndexResponse, UserRole, WhoAmIResponse,
};

use crate::api;
use crate::app::{SCROLL_PAGE_SIZE, friendly_error, relative_time_i18n, repo_route, token_option};
use crate::contexts::{ApiContext, I18nContext, ToastContext};
use crate::location::{near_window_bottom, query_param, set_location_query};
use crate::storage::{load_storage, save_storage};
use crate::urls::derive_repo_slug;

#[component]
pub(crate) fn ReposPage(lang: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();

    let whoami_token = api.token.read().clone();
    let whoami = use_resource(move || {
        let token = whoami_token.clone();
        async move {
            api::get_json::<WhoAmIResponse>(api.api_base, token_option(&token), "/whoami").await
        }
    });
    let is_admin = match &*whoami.read_unchecked() {
        Some(Ok(resp)) => resp
            .user
            .as_ref()
            .is_some_and(|u| matches!(u.role, UserRole::Admin)),
        _ => false,
    };

    let init_q = query_param("q")
        .or_else(|| load_storage("savhub.repos.q"))
        .unwrap_or_default();

    let init_q2 = init_q.clone();
    let mut search_input = use_signal(move || init_q.clone());
    let mut search_query = use_signal(move || init_q2.clone());
    let mut debounce_gen = use_signal(|| 0u32);

    // Infinite scroll state
    let mut repo_items = use_signal(Vec::<RepoSummary>::new);
    let mut has_more = use_signal(|| true);
    let mut loading = use_signal(|| false);
    let mut reset_gen = use_signal(|| 0u32);

    // Keep URL in sync with list state.
    use_effect(move || {
        let q = search_query.read().clone();
        save_storage("savhub.repos.q", &q);
        set_location_query(&[("q", &q)]);
    });

    // Reset items when search changes
    use_effect(move || {
        let _ = search_query.read();
        repo_items.set(Vec::new());
        has_more.set(true);
        {
            let v = *reset_gen.peek() + 1;
            reset_gen.set(v);
        }
    });

    let fetch_repos = move |append: bool| {
        let token = api.token.peek().clone();
        let search_val = search_query.peek().clone();
        let cur_gen = *reset_gen.peek();
        spawn(async move {
            if *loading.peek() {
                return;
            }
            loading.set(true);
            let offset = if append { repo_items.peek().len() } else { 0 } as i64;
            let mut url = format!("/repos?limit={SCROLL_PAGE_SIZE}&cursor={offset}");
            if !search_val.trim().is_empty() {
                url.push_str(&format!("&q={}", search_val.trim()));
            }
            match api::get_json::<PagedResponse<RepoSummary>>(
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
                        repo_items.write().extend(data.items);
                    } else {
                        repo_items.set(data.items);
                    }
                }
                Err(_) => has_more.set(false),
            }
            loading.set(false);
        });
    };

    use_effect(move || {
        let _ = reset_gen.read();
        fetch_repos(false);
    });

    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(300).await;
            if !*has_more.peek() || *loading.peek() {
                continue;
            }
            if near_window_bottom(200.0) {
                fetch_repos(true);
            }
        }
    });

    let t = use_context::<I18nContext>().t();
    rsx! {
        document::Title { "{t.repos_title}" }
        section { class: "section",
            div { class: "list-toolbar",
                h2 { class: "toolbar-title", "{t.repos_heading}" }
                input {
                    class: "search-input",
                    r#type: "search",
                    placeholder: "{t.search_repos}",
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
            }
            {
                let items = repo_items.read();
                if items.is_empty() && !*loading.read() {
                    rsx! {
                        div { class: "empty-state",
                            if search_query.read().is_empty() {
                                "{t.no_repos_yet}"
                            } else {
                                "{t.no_repos_match}"
                            }
                        }
                    }
                } else {
                    rsx! {
                        div { class: "list-view",
                            for repo in items.iter().cloned() {
                                RepoListRow { repo, is_admin }
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
pub(crate) fn RepoListRow(repo: RepoSummary, #[props(default = false)] is_admin: bool) -> Element {
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let toast = use_context::<ToastContext>();
    let i18n_ctx = use_context::<I18nContext>();
    let updated = relative_time_i18n(repo.updated_at, i18n_ctx.t());
    let repo_slug = derive_repo_slug(&repo.git_url);
    let git_url = repo.git_url.clone();
    let git_url_display = git_url.clone();
    let mut reindex_loading = use_signal(|| false);
    let mut reindex_done = use_signal(|| false);
    rsx! {
        div { class: "list-item",
            div { class: "list-item-main",
                h3 { Link { to: repo_route(&repo_slug), "{repo.name}" } }
                a { class: "list-item-slug", href: "{git_url_display}", target: "_blank", rel: "noreferrer", "{repo_slug}" }
                if !repo.description.is_empty() {
                    p { class: "list-item-summary", "{repo.description}" }
                }
                div { class: "list-item-meta",
                    span { "{repo.visibility:?}" }
                    span { if repo.verified { "{t.verified}" } else { "{t.community}" } }
                    span { class: "relative-time", "{updated}" }
                }
            }
            div { class: "list-item-right",
                if is_admin {
                    if *reindex_done.read() {
                        span { class: "btn-sm", "{t.reindex_submitted}" }
                    } else {
                        button {
                            class: "btn-sm",
                            disabled: *reindex_loading.read(),
                            onclick: move |_| {
                                if *reindex_loading.read() { return; }
                                reindex_loading.set(true);
                                let token = api.token.read().clone();
                                let url = git_url.clone();
                                spawn(async move {
                                    let body = SubmitIndexRequest {
                                        git_url: url,
                                        git_ref: "main".to_string(),
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
                                            reindex_done.set(true);
                                        }
                                        Err(e) => {
                                            toast.error(friendly_error(&e, t));
                                        }
                                    }
                                    reindex_loading.set(false);
                                });
                            },
                            if *reindex_loading.read() {
                                span { class: "spinner" }
                            }
                            "{t.reindex}"
                        }
                    }
                }
                span { class: "pill", "{repo.skill_count} {t.nav_skills}" }
            }
        }
    }
}
