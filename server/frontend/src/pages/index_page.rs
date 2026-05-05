use dioxus::prelude::*;
use savhub_shared::{
    IndexJobDto, IndexJobListResponse, IndexJobStatus, SubmitIndexRequest, SubmitIndexResponse,
    UserRole, WhoAmIResponse,
};

use crate::api;
use crate::app::{
    format_local_datetime_short, friendly_error, github_login_url, render_index_job_detail,
    render_ws_index_progress, short_repo_name, token_option,
};
use crate::contexts::{ApiContext, I18nContext, ToastContext, WsIndexEvents};
use crate::ws::ws_subscribe;

#[component]
pub(crate) fn IndexPage(lang: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let toast = use_context::<ToastContext>();
    let ws = use_context::<WsIndexEvents>();
    let mut index_url = use_signal(String::new);
    let mut selected_id = use_signal(|| None::<uuid::Uuid>);
    let mut refreshed_for = use_signal(|| None::<uuid::Uuid>);
    let token_val = api.token.read().clone();

    if token_val.trim().is_empty() {
        let login_href = github_login_url(api.api_base);
        return rsx! {
            document::Title { "{t.index_title}" }
            section { class: "section",
                div { class: "auth-prompt",
                    div { class: "auth-icon",
                        crate::icons::IconLock { size: 28 }
                    }
                    h2 { "{t.index_title}" }
                    p { "{t.login_before_index}" }
                    a { class: "btn-github", href: "{login_href}",
                        // GitHub SVG icon
                        svg { view_box: "0 0 16 16",
                            path { d: "M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" }
                        }
                        "{t.sign_in_github}"
                    }
                }
            }
        };
    }

    // Check admin status for showing force checkbox
    let whoami_token = token_val.clone();
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

    let mut force_index = use_signal(move || is_admin);
    // Update force_index when admin status resolves
    use_effect(move || {
        force_index.set(is_admin);
    });

    // -- Infinite-scroll job list state --
    let mut job_list = use_signal(Vec::<IndexJobDto>::new);
    let mut has_more_jobs = use_signal(|| true);
    let mut loading_jobs = use_signal(|| false);
    let mut job_list_error = use_signal(|| None::<String>);
    let page_size: i64 = 20;

    // Fetch a page of jobs. `append` = true appends, false reloads from offset 0.
    // All captured values are Copy (Signals + &'static str), so this closure is Copy.
    let fetch_jobs = move |append: bool| {
        let token = api.token.peek().clone();
        spawn(async move {
            if token.trim().is_empty() || *loading_jobs.peek() {
                return;
            }
            // Only show loading spinner on initial load, not background refreshes
            let show_loading = append || job_list.peek().is_empty();
            if show_loading {
                loading_jobs.set(true);
            }
            let offset = if append {
                job_list.peek().len() as i64
            } else {
                0
            };
            let path = format!("/index/list?limit={page_size}&offset={offset}");
            match api::get_json::<IndexJobListResponse>(api.api_base, token_option(&token), &path)
                .await
            {
                Ok(data) => {
                    has_more_jobs.set(data.has_more);
                    if append {
                        let mut list = job_list.write();
                        for job in data.jobs {
                            if !list.iter().any(|j| j.id == job.id) {
                                list.push(job);
                            }
                        }
                    } else {
                        job_list.set(data.jobs);
                    }
                    job_list_error.set(None);
                }
                Err(e) => job_list_error.set(Some(e)),
            }
            loading_jobs.set(false);
        });
    };

    // Initial load
    use_effect(move || {
        fetch_jobs(false);
    });

    // Poll first page every 5s while a job is selected
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(5_000).await;
            if selected_id.read().is_some() {
                fetch_jobs(false);
            }
        }
    });

    // WS event for selected job (reading ws.events creates reactive subscription)
    let sel_id = *selected_id.read();
    let ws_event = sel_id.and_then(|id| {
        let events = ws.events.read();
        events.get(&id.to_string()).cloned()
    });

    // Refresh job list when a tracked job completes or fails
    if let Some(ref evt) = ws_event
        && let Some(id) = sel_id
        && (evt.status == "completed" || evt.status == "failed")
        && *refreshed_for.read() != Some(id)
    {
        refreshed_for.set(Some(id));
        fetch_jobs(false);
    }

    rsx! {
        document::Title { "{t.index_title}" }
        section { class: "section",
            div { class: "publish-bar",
                input {
                    class: "publish-input",
                    r#type: "text",
                    value: "{index_url}",
                    placeholder: "user/repo",
                    oninput: move |e| index_url.set(e.value()),
                }
                if is_admin {
                    label { class: "checkbox-label",
                        input {
                            r#type: "checkbox",
                            checked: *force_index.read(),
                            oninput: move |e: Event<FormData>| force_index.set(e.value() == "true"),
                        }
                        " Force"
                    }
                }
                button {
                    class: "primary",
                    onclick: move |_| {
                        let token = api.token.read().clone();
                        if token.trim().is_empty() {
                            toast.error(t.login_before_index.to_string());
                            return;
                        }
                        let raw = index_url.read().clone();
                        let raw = raw.trim().to_string();
                        if raw.is_empty() {
                            toast.error(t.git_url_required.to_string());
                            return;
                        }
                        let force = *force_index.read();
                        // Auto-expand "user/repo" to full GitHub URL
                        let url = if !raw.contains("://") && !raw.contains("git@") {
                            format!("https://github.com/{raw}")
                        } else {
                            raw
                        };
                        spawn(async move {
                            let body = SubmitIndexRequest {
                                git_url: url,
                                git_ref: "main".to_string(),
                                git_subdir: ".".to_string(),
                                repo_slug: None,
                                force,
                            };
                            match api::post_json::<_, SubmitIndexResponse>(
                                api.api_base,
                                token_option(&token),
                                "/index",
                                &body,
                            ).await {
                                Ok(resp) => {
                                    selected_id.set(Some(resp.job_id));
                                    refreshed_for.set(None);
                                    ws_subscribe(&resp.job_id.to_string());
                                    index_url.set(String::new());
                                    fetch_jobs(false);
                                }
                                Err(e) => toast.error(friendly_error(&e, t)),
                            }
                        });
                    },
                    "{t.index_submit}"
                }
            }
        }

        div { class: "publish-layout",
            aside {
                class: "publish-sidebar",
                onscroll: move |_| {
                    if !*has_more_jobs.peek() || *loading_jobs.peek() {
                        return;
                    }
                    if let Some(el) = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.query_selector(".publish-sidebar").ok().flatten())
                    {
                        let scroll_top = el.scroll_top() as f64;
                        let client_h = el.client_height() as f64;
                        let scroll_h = el.scroll_height() as f64;
                        if scroll_top + client_h + 50.0 >= scroll_h {
                            fetch_jobs(true);
                        }
                    }
                },
                {
                    let jobs = job_list.read();
                    if jobs.is_empty() && !*loading_jobs.read() {
                        rsx! { p { class: "muted", "{t.no_index_jobs}" } }
                    } else {
                        rsx! {
                            for job in jobs.iter() {
                                {
                                    let job_id = job.id;
                                    let is_selected = sel_id == Some(job_id);
                                    let icon_class = match job.status {
                                        IndexJobStatus::Completed => "scan-job-icon done",
                                        IndexJobStatus::Failed => "scan-job-icon failed",
                                        IndexJobStatus::Running => "scan-job-icon running",
                                        IndexJobStatus::Superseded => "scan-job-icon failed",
                                        IndexJobStatus::Pending => "scan-job-icon pending",
                                    };
                                    let item_class = if is_selected {
                                        "scan-job-item selected"
                                    } else {
                                        "scan-job-item"
                                    };
                                    let name = short_repo_name(&job.git_url);
                                    let time_short = format_local_datetime_short(job.created_at);
                                    rsx! {
                                        div {
                                            class: "{item_class}",
                                            onclick: move |_| {
                                                selected_id.set(Some(job_id));
                                                ws_subscribe(&job_id.to_string());
                                            },
                                            span { class: "{icon_class}" }
                                            div { class: "scan-job-item-info",
                                                span { class: "scan-job-item-name", "{name}" }
                                                span { class: "scan-job-item-time muted", "{time_short}" }
                                            }
                                        }
                                    }
                                }
                            }
                            if *loading_jobs.read() {
                                p { class: "muted center", "{t.loading}" }
                            }
                        }
                    }
                }
                if let Some(ref err) = *job_list_error.read() {
                    { let msg = friendly_error(err, t); rsx! { p { class: "error", "{msg}" } } }
                }
            }
            section { class: "publish-detail",
                {
                    let has_ws = ws_event.as_ref().is_some_and(|e| e.status == "running" || e.status == "pending" || e.status == "completed" || e.status == "failed");
                    // Look up git_url/git_ref from job list for WS-driven view
                    let ws_job_info = sel_id.and_then(|id| {
                        let jobs = job_list.read();
                        jobs.iter().find(|j| j.id == id).map(|j| (j.git_url.clone(), j.git_ref.clone()))
                    });
                    if has_ws {
                        render_ws_index_progress(
                            ws_event.as_ref().unwrap(),
                            t,
                            ws_job_info.as_ref().map(|(u, _)| u.as_str()),
                            ws_job_info.as_ref().map(|(_, r)| r.as_str()),
                        )
                    } else if let Some(id) = sel_id {
                        let jobs = job_list.read();
                        match jobs.iter().find(|j| j.id == id) {
                            Some(job) => render_index_job_detail(job, t),
                            None => {
                                if let Some(ref evt) = ws_event {
                                    render_ws_index_progress(evt, t, None, None)
                                } else {
                                    rsx! { p { class: "muted", "{t.waiting_index}" } }
                                }
                            }
                        }
                    } else {
                        rsx! { p { class: "muted", "{t.select_index_job}" } }
                    }
                }
            }
        }
    }
}
