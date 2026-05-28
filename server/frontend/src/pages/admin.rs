use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::{
    AdminActionResponse, CreateIndexRuleRequest, IndexRuleDto, IndexRuleListResponse,
    ManagementSummaryResponse, RoleUpdateResponse, SetUserRoleRequest, UpdateIndexRuleRequest,
    UserListResponse, UserRole, WhoAmIResponse,
};

use crate::api;
use crate::app::{Route, SCROLL_PAGE_SIZE, format_local_datetime, token_option, url_lang};
use crate::contexts::{AdminTabCtx, ApiContext, I18nContext};
use crate::i18n::T;
use crate::location::near_window_bottom;
use crate::pages::widgets::StatTile;

#[component]
pub(crate) fn ManagementPage(lang: String, tab: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let mut admin_tab_ctx = use_context::<AdminTabCtx>();
    // Sync the route tab param into the shared signal
    let tab_static: &'static str = match tab.as_str() {
        "users" => "users",
        "index_rules" => "index_rules",
        "all_jobs" => "all_jobs",
        _ => "overview",
    };
    if *admin_tab_ctx.tab.read() != tab_static {
        admin_tab_ctx.tab.set(tab_static);
    }
    let token = api.token.read().clone();
    let whoami_token = token.clone();
    let summary_token = token.clone();
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
    if !is_admin {
        return rsx! {
            section { class: "section",
                h2 { "{t.access_denied}" }
                p { "{t.access_denied_desc}" }
            }
        };
    }
    let summary = use_resource(move || {
        let token = summary_token.clone();
        async move {
            api::get_json::<ManagementSummaryResponse>(
                api.api_base,
                token_option(&token),
                "/management/summary",
            )
            .await
        }
    });

    rsx! {
        document::Title { "{t.management_title}" }
        section { class: "section",
            match tab_static {
                "overview" => rsx! {
                    {render_management_summary(&summary, t)}
                },
                "users" => rsx! {
                    AdminUsersPage { lang: lang.clone() }
                },
                "index_rules" => rsx! {
                    AdminIndexRulesPage { lang: lang.clone() }
                },
                _ => rsx! {
                    {render_management_summary(&summary, t)}
                },
            }
        }
    }
}

#[component]
pub(crate) fn AdminOverviewPage(lang: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let token = api.token.read().clone();
    let summary = use_resource(move || {
        let token = token.clone();
        async move {
            api::get_json::<ManagementSummaryResponse>(
                api.api_base,
                token_option(&token),
                "/management/summary",
            )
            .await
        }
    });
    rsx! {
        document::Title { "{t.management_title}" }
        section { class: "section",
            {render_management_summary(&summary, t)}
        }
    }
}

#[component]
pub(crate) fn AdminUsersPage(lang: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let token = api.token.read().clone();
    let mut action_msg = use_signal(String::new);
    let mut users = use_resource(move || {
        let token = token.clone();
        async move {
            api::get_json::<UserListResponse>(api.api_base, token_option(&token), "/users?limit=50")
                .await
        }
    });

    rsx! {
        div { class: "panel",
            h2 { "{t.all_users}" }
            if !action_msg.read().is_empty() {
                p { class: "notice", "{action_msg}" }
            }
            match &*users.read_unchecked() {
                Some(Ok(data)) => rsx! {
                    div { class: "admin-table",
                        div { class: "admin-table-header",
                            span { "{t.handle}" }
                            span { "{t.role}" }
                            span { "{t.nav_skills}" }
                            span { "{t.actions}" }
                        }
                        for item in data.items.iter() {
                            {
                                let user_id = item.user.id;
                                let handle = item.user.handle.clone();
                                let current_role = item.user.role;
                                let role_str = match current_role {
                                    UserRole::Admin => "admin",
                                    UserRole::Moderator => "moderator",
                                    UserRole::User => "user",
                                };
                                let handle_for_msg = handle.clone();
                                let handle_for_link = handle.clone();
                                rsx! {
                                    div { class: "admin-table-row",
                                        span {
                                            Link {
                                                to: Route::UserPage { lang: url_lang(), handle: handle.clone() },
                                                "@{handle}"
                                            }
                                        }
                                        span {
                                            select {
                                                class: "role-select",
                                                value: "{role_str}",
                                                onchange: move |e: Event<FormData>| {
                                                    let new_role = match e.value().as_str() {
                                                        "admin" => UserRole::Admin,
                                                        "moderator" => UserRole::Moderator,
                                                        _ => UserRole::User,
                                                    };
                                                    let token = api.token.read().clone();
                                                    let h = handle_for_msg.clone();
                                                    spawn(async move {
                                                        let url = format!("/management/users/{}/role", user_id);
                                                        let body = SetUserRoleRequest { role: new_role };
                                                        match api::post_json::<_, RoleUpdateResponse>(
                                                            api.api_base,
                                                            token_option(&token),
                                                            &url,
                                                            &body,
                                                        ).await {
                                                            Ok(_) => {
                                                                action_msg.set(format!("Role updated for @{}", h));
                                                                users.restart();
                                                            }
                                                            Err(e) => action_msg.set(format!("Error: {e}")),
                                                        }
                                                    });
                                                },
                                                option { value: "user", selected: role_str == "user", "User" }
                                                option { value: "moderator", selected: role_str == "moderator", "Moderator" }
                                                option { value: "admin", selected: role_str == "admin", "Admin" }
                                            }
                                        }
                                        span { "{item.skill_count}" }
                                        span { class: "admin-actions",
                                            Link {
                                                to: Route::UserPage { lang: url_lang(), handle: handle_for_link },
                                                class: "secondary",
                                                "{t.view}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                Some(Err(error)) => rsx! { p { class: "error", "{error}" } },
                None => rsx! { p { "{t.loading_users}" } },
            }
        }
    }
}

#[component]
pub(crate) fn AdminIndexRulesPage(lang: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let mut action_msg = use_signal(String::new);
    let mut show_create = use_signal(|| false);
    let mut new_repo_url = use_signal(String::new);
    let mut new_path = use_signal(String::new);
    let mut new_strategy = use_signal(|| "smart".to_string());
    let mut new_desc = use_signal(String::new);
    let mut editing_id = use_signal(|| None::<uuid::Uuid>);
    let mut edit_repo_url = use_signal(String::new);
    let mut edit_path = use_signal(String::new);
    let mut edit_strategy = use_signal(String::new);
    let mut edit_desc = use_signal(String::new);

    let mut search_input = use_signal(String::new);
    let mut search_query = use_signal(String::new);
    let mut debounce_gen = use_signal(|| 0u32);

    // Infinite scroll state
    let mut rule_items = use_signal(Vec::<IndexRuleDto>::new);
    let mut has_more = use_signal(|| true);
    let mut loading = use_signal(|| false);
    let mut reset_gen = use_signal(|| 0u32);
    let mut rules_error = use_signal(|| None::<String>);

    // Reset items when search changes
    use_effect(move || {
        let _ = search_query.read();
        rule_items.set(Vec::new());
        has_more.set(true);
        {
            let v = *reset_gen.peek() + 1;
            reset_gen.set(v);
        }
    });

    let fetch_rules = move |append: bool| {
        let token = api.token.peek().clone();
        let search_val = search_query.peek().clone();
        let cur_gen = *reset_gen.peek();
        spawn(async move {
            if *loading.peek() {
                return;
            }
            loading.set(true);
            let offset = if append { rule_items.peek().len() } else { 0 } as i64;
            let mut url =
                format!("/management/index-rules?limit={SCROLL_PAGE_SIZE}&cursor={offset}");
            if !search_val.is_empty() {
                url.push_str(&format!("&q={search_val}"));
            }
            match api::get_json::<IndexRuleListResponse>(api.api_base, token_option(&token), &url)
                .await
            {
                Ok(data) => {
                    if *reset_gen.peek() != cur_gen {
                        loading.set(false);
                        return;
                    }
                    has_more.set(data.next_cursor.is_some());
                    if append {
                        rule_items.write().extend(data.rules);
                    } else {
                        rule_items.set(data.rules);
                    }
                    rules_error.set(None);
                }
                Err(e) => {
                    has_more.set(false);
                    rules_error.set(Some(e));
                }
            }
            loading.set(false);
        });
    };

    use_effect(move || {
        let _ = reset_gen.read();
        fetch_rules(false);
    });

    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(300).await;
            if !*has_more.peek() || *loading.peek() {
                continue;
            }
            if near_window_bottom(200.0) {
                fetch_rules(true);
            }
        }
    });

    // Provide a way to reload rules after create/edit/delete actions
    let mut reload_rules = move || {
        rule_items.set(Vec::new());
        has_more.set(true);
        {
            let v = *reset_gen.peek() + 1;
            reset_gen.set(v);
        }
    };

    rsx! {
        div { class: "panel",
            div { class: "list-toolbar",
                h2 { class: "toolbar-title", "{t.index_rules}" }
                input {
                    class: "search-input",
                    r#type: "search",
                    placeholder: "{t.search_index_rules}",
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
                button {
                    class: "primary",
                    onclick: move |_| show_create.set(!show_create()),
                    if *show_create.read() { "{t.cancel}" } else { "{t.add_rule}" }
                }
            }

            if *show_create.read() {
                div { class: "inline-form",
                    input {
                        value: "{new_repo_url}",
                        placeholder: "{t.git_url}",
                        oninput: move |e| new_repo_url.set(e.value()),
                    }
                    input {
                        value: "{new_path}",
                        placeholder: "{t.add_rule_path_placeholder}",
                        oninput: move |e| new_path.set(e.value()),
                    }
                    select {
                        value: "{new_strategy}",
                        onchange: move |e| new_strategy.set(e.value()),
                        option { value: "smart", "Smart" }
                        option { value: "each_dir_as_flock", "Each Dir as Flock" }
                    }
                    input {
                        value: "{new_desc}",
                        placeholder: "{t.add_rule_desc_placeholder}",
                        oninput: move |e| new_desc.set(e.value()),
                    }
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let repo_url = new_repo_url.read().clone();
                            let path_regex = new_path.read().clone();
                            let strategy = new_strategy.read().clone();
                            let description = new_desc.read().clone();
                            let token = api.token.read().clone();
                            spawn(async move {
                                let body = CreateIndexRuleRequest {
                                    repo_url,
                                    path_regex,
                                    strategy,
                                    description,
                                };
                                match api::post_json::<_, IndexRuleDto>(
                                    api.api_base,
                                    token_option(&token),
                                    "/management/index-rules",
                                    &body,
                                ).await {
                                    Ok(_) => {
                                        action_msg.set("Rule created.".to_string());
                                        new_repo_url.set(String::new());
                                        new_path.set(String::new());
                                        new_strategy.set("smart".to_string());
                                        new_desc.set(String::new());
                                        show_create.set(false);
                                        reload_rules();
                                    }
                                    Err(e) => action_msg.set(format!("Error: {e}")),
                                }
                            });
                        },
                        "{t.add_rule}"
                    }
                }
            }

            if !action_msg.read().is_empty() {
                p { class: "notice", "{action_msg}" }
            }

            {
                let items = rule_items.read();
                if let Some(ref error) = *rules_error.read() {
                    rsx! { p { class: "error", "{error}" } }
                } else if items.is_empty() && !*loading.read() {
                    rsx! { p { class: "muted", "{t.no_index_rules}" } }
                } else {
                    rsx! {
                        div { class: "admin-table admin-table--rules",
                            div { class: "admin-table-header",
                                span { "{t.git_url}" }
                                span { "{t.path_regex}" }
                                span { "{t.strategy}" }
                                span { "{t.description}" }
                                span { "{t.actions}" }
                            }
                            for rule in items.iter() {
                                {
                                    let rule_id = rule.id;
                                    let is_editing = editing_id.read().is_some_and(|id| id == rule_id);
                                    if is_editing {
                                        rsx! {
                                            div { class: "admin-table-row",
                                                span {
                                                    input {
                                                        value: "{edit_repo_url}",
                                                        oninput: move |e| edit_repo_url.set(e.value()),
                                                    }
                                                }
                                                span {
                                                    input {
                                                        value: "{edit_path}",
                                                        oninput: move |e| edit_path.set(e.value()),
                                                    }
                                                }
                                                span {
                                                    select {
                                                        value: "{edit_strategy}",
                                                        onchange: move |e| edit_strategy.set(e.value()),
                                                        option { value: "smart", "Smart" }
                                                        option { value: "each_dir_as_flock", "Each Dir as Flock" }
                                                    }
                                                }
                                                span {
                                                    input {
                                                        value: "{edit_desc}",
                                                        oninput: move |e| edit_desc.set(e.value()),
                                                    }
                                                }
                                                span { class: "admin-actions",
                                                    button {
                                                        class: "primary",
                                                        onclick: move |_| {
                                                            let repo_url = edit_repo_url.read().clone();
                                                            let path_regex = edit_path.read().clone();
                                                            let strategy = edit_strategy.read().clone();
                                                            let description = edit_desc.read().clone();
                                                            let token = api.token.read().clone();
                                                            spawn(async move {
                                                                let body = UpdateIndexRuleRequest {
                                                                    repo_url: Some(repo_url),
                                                                    path_regex: Some(path_regex),
                                                                    strategy: Some(strategy),
                                                                    description: Some(description),
                                                                };
                                                                let url = format!("/management/index-rules/{}", rule_id);
                                                                match api::post_json::<_, IndexRuleDto>(
                                                                    api.api_base,
                                                                    token_option(&token),
                                                                    &url,
                                                                    &body,
                                                                ).await {
                                                                    Ok(_) => {
                                                                        action_msg.set("Rule updated.".to_string());
                                                                        editing_id.set(None);
                                                                        reload_rules();
                                                                    }
                                                                    Err(e) => action_msg.set(format!("Error: {e}")),
                                                                }
                                                            });
                                                        },
                                                        "{t.save}"
                                                    }
                                                    button {
                                                        class: "ghost",
                                                        onclick: move |_| editing_id.set(None),
                                                        "{t.cancel}"
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        let url_trimmed = rule.repo_url
                                            .strip_prefix("https://").unwrap_or(&rule.repo_url);
                                        let repo_url_display = url_trimmed
                                            .strip_suffix(".git").unwrap_or(url_trimmed)
                                            .to_string();
                                        let path_display = rule.path_regex.clone();
                                        let strategy_display = match rule.strategy.as_str() {
                                            "each_dir_as_flock" | "subdirs_as_flocks" => "Each Dir as Flock",
                                            _ => "Smart",
                                        }.to_string();
                                        let desc_display = rule.description.clone();
                                        let edit_ru = rule.repo_url.clone();
                                        let edit_p = rule.path_regex.clone();
                                        let edit_s = rule.strategy.clone();
                                        let edit_d = rule.description.clone();
                                        rsx! {
                                            div { class: "admin-table-row",
                                                span { "{repo_url_display}" }
                                                span { "{path_display}" }
                                                span { class: "pill", "{strategy_display}" }
                                                span { "{desc_display}" }
                                                span { class: "admin-actions",
                                                    button {
                                                        class: "secondary",
                                                        onclick: move |_| {
                                                            editing_id.set(Some(rule_id));
                                                            edit_repo_url.set(edit_ru.clone());
                                                            edit_path.set(edit_p.clone());
                                                            edit_strategy.set(edit_s.clone());
                                                            edit_desc.set(edit_d.clone());
                                                        },
                                                        "{t.edit}"
                                                    }
                                                    button {
                                                        class: "ghost",
                                                        onclick: move |_| {
                                                            let token = api.token.read().clone();
                                                            spawn(async move {
                                                                let url = format!("/management/index-rules/{}", rule_id);
                                                                match api::delete_json::<AdminActionResponse>(
                                                                    api.api_base,
                                                                    token_option(&token),
                                                                    &url,
                                                                ).await {
                                                                    Ok(resp) => {
                                                                        action_msg.set(resp.message);
                                                                        reload_rules();
                                                                    }
                                                                    Err(e) => action_msg.set(format!("Error: {e}")),
                                                                }
                                                            });
                                                        },
                                                        "{t.delete}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
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

fn render_management_summary(
    resource: &Resource<Result<ManagementSummaryResponse, String>>,
    t: &T,
) -> Element {
    match &*resource.read_unchecked() {
        Some(Ok(summary)) => rsx! {
            div { class: "panel",
                h2 { "{t.catalog_totals}" }
                div { class: "stats-grid",
                    StatTile { label: t.users, value: summary.counts.users }
                    StatTile { label: t.nav_repos, value: summary.counts.repos }
                    StatTile { label: t.flocks, value: summary.counts.flocks }
                    StatTile { label: t.nav_skills, value: summary.counts.skills }
                    StatTile { label: t.versions, value: summary.counts.versions }
                    StatTile { label: t.comments, value: summary.counts.comments }
                }

                h2 { "{t.ai_usage_title}" }
                if summary.ai_usage.is_empty() {
                    p { class: "empty-state", "{t.ai_no_usage}" }
                } else {
                    table { class: "ai-usage-table",
                        thead {
                            tr {
                                th { "Task" }
                                th { "Model" }
                                th { style: "text-align: right;", "{t.ai_calls}" }
                                th { style: "text-align: right;", "{t.ai_prompt_tokens}" }
                                th { style: "text-align: right;", "{t.ai_completion_tokens}" }
                                th { style: "text-align: right;", "{t.ai_total_tokens}" }
                            }
                        }
                        tbody {
                            for item in summary.ai_usage.iter() {
                                {
                                    let task_label = match item.task_type.as_str() {
                                        "flock_metadata" => t.ai_task_flock_metadata,
                                        "security_scan" => t.ai_task_security_scan,
                                        other => other,
                                    };
                                    let badge_class = match item.task_type.as_str() {
                                        "flock_metadata" => "task-badge flock-metadata",
                                        "security_scan" => "task-badge security-scan",
                                        _ => "task-badge",
                                    };
                                    rsx! {
                                        tr {
                                            td { span { class: "{badge_class}", "{task_label}" } }
                                            td { code { "{item.model}" } }
                                            td { class: "num", "{item.call_count}" }
                                            td { class: "num", "{format_token_count(item.total_prompt_tokens)}" }
                                            td { class: "num", "{format_token_count(item.total_completion_tokens)}" }
                                            td { class: "num", "{format_token_count(item.total_tokens)}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                h2 { "{t.audit_log}" }
                ul { class: "dense-list",
                    for log in summary.audit_logs.iter() {
                        {
                            let actor_name = log.actor.as_ref()
                                .map(|a| a.display_name.as_deref().unwrap_or(&a.handle))
                                .unwrap_or("anonymous");
                            let ts = format_local_datetime(log.created_at);
                            rsx! {
                                li { "{ts} · {actor_name} · {log.action} · {log.target_type}" }
                            }
                        }
                    }
                }
            }
        },
        Some(Err(error)) => rsx! { p { class: "error", "{error}" } },
        None => rsx! { p { "{t.loading_management}" } },
    }
}

fn format_token_count(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}
