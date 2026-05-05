#![cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, unused_imports, unused_mut)
)]

use std::sync::OnceLock;

use dioxus::prelude::*;
use dioxus_router::{Link, Outlet, Routable, Router};
use savhub_shared::{
    BundleMetadata, BundleSourceKind, CatalogSource, CommentDto, FlockSummary, IndexJobDto,
    IndexJobStatus, PagedResponse, SecurityStatus, SubmitIndexRequest, SubmitIndexResponse,
    UserRole, WhoAmIResponse,
};

use crate::api;
use crate::contexts::{
    AdminTabCtx, ApiContext, I18nContext, IndexProgressEvent, InitialAuthState, StoredAuthSession,
    ToastContext, ToastKind, WsIndexEvents,
};
use crate::i18n::{Lang, T};
use crate::pages::admin::{AdminIndexRulesPage, AdminOverviewPage, AdminUsersPage};
use crate::pages::cards::FlockCard;
use crate::pages::docs::DocsPage;
use crate::pages::download::DownloadPage;
use crate::pages::flock::FlockPage;
use crate::pages::home::Home;
use crate::pages::index_page::IndexPage;
use crate::pages::not_found::NotFound;
use crate::pages::repo::RepoPage;
use crate::pages::repos_list::ReposPage;
use crate::pages::skill::SkillPage;
use crate::pages::skills_list::SkillsPage;
use crate::pages::user::UserPage;
use crate::location::{
    clear_location_hash, clear_location_query, current_hash, current_origin, current_page_url,
    current_query, parse_hash_params,
};
use crate::storage::{browser_storage, load_storage, remove_storage, save_storage};
use crate::urls::strip_url_scheme_keep_git;
use crate::ws::start_ws_connection;

const SAVHUB_LOGO: Asset = asset!("/assets/savhub.svg");
const SAVHUB_FAVICON: Asset = asset!("/assets/savhub.ico");
const BAMBOO_DECOR: Asset = asset!("/assets/bamboo_decor.svg");
const AUTH_TOKEN_STORAGE_KEY: &str = "savhub.auth_token";
const AUTH_SESSIONS_STORAGE_KEY: &str = "savhub.auth_sessions";
pub(crate) const LANG_STORAGE_KEY: &str = "savhub.lang";
pub(crate) const SCROLL_PAGE_SIZE: i64 = 20;

const ADMIN_MODE_STORAGE_KEY: &str = "savhub.admin_mode";
pub(crate) const CLIENT_REPO_URL: &str = "https://github.com/savhub-ai/savhub";
pub(crate) const CLIENT_RELEASES_URL: &str = "https://github.com/savhub-ai/savhub/releases";
pub(crate) const CLIENT_LATEST_RELEASE_URL: &str =
    "https://github.com/savhub-ai/savhub/releases/latest";
pub(crate) const INSTALL_SH_URL: &str =
    "https://raw.githubusercontent.com/savhub-ai/savhub/main/scripts/install.sh";
pub(crate) const INSTALL_PS1_URL: &str =
    "https://raw.githubusercontent.com/savhub-ai/savhub/main/scripts/install.ps1";

/// Get the current language code from I18nContext for constructing Route links.
pub(crate) fn url_lang() -> String {
    use_context::<I18nContext>().lang.read().code().to_string()
}

/// Build a `Route::RepoPage` from a combined slug like `"github.com/owner/name"`.
pub(crate) fn repo_route(slug: &str) -> Route {
    let parts: Vec<&str> = slug.splitn(3, '/').collect();
    Route::RepoPage {
        lang: url_lang(),
        domain: parts.first().unwrap_or(&"").to_string(),
        owner: parts.get(1).unwrap_or(&"").to_string(),
        name: parts.get(2).unwrap_or(&"").to_string(),
    }
}

#[component]
fn ToastOverlay() -> Element {
    let toast = use_context::<ToastContext>();
    let items = toast.items.read().clone();

    if items.is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "toast-container",
            for item in items.iter() {
                {
                    let id = item.id;
                    let css = match item.kind {
                        ToastKind::Error => "toast toast-error",
                        ToastKind::Success => "toast toast-success",
                        ToastKind::Info => "toast toast-info",
                    };
                    let msg = item.message.clone();
                    rsx! {
                        div { class: "{css}",
                            span { class: "toast-body", "{msg}" }
                            button {
                                class: "toast-close",
                                onclick: move |_| toast.dismiss(id),
                                crate::icons::IconX { size: 14 }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn api_base() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| {
        option_env!("SAVHUB_API_BASE")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                current_origin()
                    .map(|origin| format!("{origin}/api/v1"))
                    .unwrap_or_else(|| "http://127.0.0.1:5006/api/v1".to_string())
            })
    })
}

#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
pub(crate) enum Route {
    #[layout(AppShell)]
        #[route("/:lang")]
        Home { lang: String },
        #[route("/:lang/repos")]
        ReposPage { lang: String },
        #[route("/:lang/repos/:domain/:owner/:name")]
        RepoPage { lang: String, domain: String, owner: String, name: String },
        #[route("/:lang/flocks/:id")]
        FlockPage { lang: String, id: String },
        #[route("/:lang/skills")]
        SkillsPage { lang: String },
        #[route("/:lang/skills/:id")]
        SkillPage { lang: String, id: String },
        #[route("/:lang/download")]
        DownloadPage { lang: String },
        #[route("/:lang/index")]
        IndexPage { lang: String },
        #[route("/:lang/users/:handle")]
        UserPage { lang: String, handle: String },
        #[route("/:lang/overview")]
        AdminOverviewPage { lang: String },
        #[route("/:lang/users")]
        AdminUsersPage { lang: String },
        #[route("/:lang/index-rules")]
        AdminIndexRulesPage { lang: String },
        #[route("/:lang/docs/:..path")]
        DocsPage { lang: String, path: Vec<String> },
    #[end_layout]
    #[route("/")]
    RootRedirect {},
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[component]
pub fn App() -> Element {
    let initial = initial_auth_state().clone();
    let initial_token = initial.token.clone();
    let initial_notice = initial.notice.clone();
    let token = use_signal(move || initial_token.clone());
    let auth_notice = use_signal(move || initial_notice.clone());
    use_context_provider(|| ApiContext {
        api_base: api_base(),
        token,
        auth_notice,
    });
    let initial_lang = Lang::from_code(&load_storage(LANG_STORAGE_KEY).unwrap_or_default());
    let lang = use_signal(move || initial_lang);
    use_context_provider(|| I18nContext { lang });
    let toast_items = use_signal(Vec::new);
    let toast_counter = use_signal(|| 0u64);
    use_context_provider(|| ToastContext {
        items: toast_items,
        counter: toast_counter,
    });
    let ws_events_signal = use_signal(std::collections::BTreeMap::new);
    let ws_message_log_signal = use_signal(std::collections::BTreeMap::new);
    let ws_events = use_context_provider(|| WsIndexEvents {
        events: ws_events_signal,
        message_log: ws_message_log_signal,
    });

    // Global WebSocket connection
    use_effect(move || {
        start_ws_connection(ws_events);
    });

    rsx! {
        document::Link { rel: "icon", href: SAVHUB_FAVICON }
        style { {include_str!("./style.css")} }
        style { {format!("body::before, body::after {{ --bamboo-bg: url('{}'); }}", BAMBOO_DECOR)} }
        ToastOverlay {}
        Router::<Route> {}
    }
}

#[component]
fn RootRedirect() -> Element {
    #[cfg(target_arch = "wasm32")]
    {
        let saved = load_storage(LANG_STORAGE_KEY).unwrap_or_else(|| "en".to_string());
        let lang = if saved == "zh" { "zh" } else { "en" };
        if let Some(w) = web_sys::window() {
            let _ = w.location().replace(&format!("/{lang}"));
        }
    }
    rsx! {}
}

#[component]
fn AppShell() -> Element {
    let api = use_context::<ApiContext>();
    let mut i18n_ctx = use_context::<I18nContext>();

    // Sync I18nContext.lang from the URL prefix (/:lang/...)
    // If the URL doesn't start with a valid lang code, redirect with saved lang.
    #[cfg(target_arch = "wasm32")]
    {
        let current_path = web_sys::window()
            .and_then(|w| w.location().pathname().ok())
            .unwrap_or_else(|| "/en".to_string());
        let first_segment = current_path.split('/').nth(1).unwrap_or("");
        if first_segment == "en" || first_segment == "zh" {
            let new_lang = Lang::from_code(first_segment);
            if *i18n_ctx.lang.read() != new_lang {
                i18n_ctx.lang.set(new_lang);
                save_storage(LANG_STORAGE_KEY, new_lang.code());
            }
        } else {
            // No valid lang prefix – prepend saved (or default) lang and redirect
            let saved_lang = load_storage(LANG_STORAGE_KEY)
                .filter(|s| s == "en" || s == "zh")
                .unwrap_or_else(|| "en".to_string());
            let new_url = format!("/{saved_lang}{current_path}");
            if let Some(w) = web_sys::window() {
                let _ = w.location().replace(&new_url);
            }
            return rsx! {};
        }
    }

    let t = i18n_ctx.t();
    let mut token_signal = api.token;
    let mut auth_notice_signal = api.auth_notice;
    let mut saved_sessions = use_signal(load_auth_sessions);
    let token_value = api.token.read().clone();
    let auth_notice_value = api.auth_notice.read().clone();
    let whoami_token = token_value.clone();
    let mut whoami = use_resource(move || {
        let token_value = whoami_token.clone();
        async move {
            api::get_json::<WhoAmIResponse>(api.api_base, token_option(&token_value), "/whoami")
                .await
        }
    });

    #[derive(Clone)]
    enum AuthState {
        Loading,
        Anonymous,
        Error(String),
        LoggedIn {
            handle: String,
            display_name: Option<String>,
            role: UserRole,
            avatar_url: Option<String>,
        },
    }

    let whoami_snapshot = (*whoami.read_unchecked()).clone();
    {
        let token_value = token_value.clone();
        let mut saved_sessions = saved_sessions;
        let mut token_signal = token_signal;
        let mut auth_notice_signal = auth_notice_signal;
        use_effect(move || {
            if token_value.trim().is_empty() {
                return;
            }
            match whoami_snapshot.clone() {
                Some(Ok(payload)) => {
                    if let Some(user) = payload.user {
                        let next = upsert_auth_session(&token_value, &user);
                        if *saved_sessions.read() != next {
                            saved_sessions.set(next);
                        }
                    }
                }
                Some(Err(error)) if error.starts_with("401") => {
                    let next = remove_auth_session(&token_value);
                    if *saved_sessions.read() != next {
                        saved_sessions.set(next);
                    }
                    clear_stored_auth_token();
                    token_signal.set(String::new());
                    auth_notice_signal.set(String::new());
                }
                _ => {}
            }
        });
    }

    let auth_state = match &*whoami.read_unchecked() {
        Some(Ok(payload)) => {
            if let Some(user) = payload.user.as_ref() {
                AuthState::LoggedIn {
                    handle: user.handle.clone(),
                    display_name: user.display_name.clone(),
                    role: user.role,
                    avatar_url: user.avatar_url.clone(),
                }
            } else {
                AuthState::Anonymous
            }
        }
        Some(Err(error)) => {
            if error.starts_with("401") {
                AuthState::Anonymous
            } else {
                AuthState::Error(error.clone())
            }
        }
        None => AuthState::Loading,
    };

    let is_admin = matches!(
        &auth_state,
        AuthState::LoggedIn {
            role: UserRole::Admin,
            ..
        }
    );
    let login_href = github_login_url(api.api_base);
    let mut dropdown_open = use_signal(|| false);
    let saved_accounts = saved_sessions.read().clone();
    let init_admin_mode = load_storage(ADMIN_MODE_STORAGE_KEY).as_deref() == Some("true");
    let mut admin_mode = use_signal(move || init_admin_mode);
    let admin_tab: Signal<&'static str> = use_signal(|| "overview");
    use_context_provider(|| AdminTabCtx { tab: admin_tab });

    rsx! {
        div { class: "app-shell",
            header { class: "topbar",
                div { class: "brand",
                    Link { to: Route::Home { lang: url_lang() }, class: "brand-link",
                        img { src: SAVHUB_LOGO, alt: "{t.brand_name}", class: "brand-logo" }
                        span { "{t.brand_name}" }
                    }
                }
                nav { class: "nav",
                    if is_admin && *admin_mode.read() {
                        Link { to: Route::AdminOverviewPage { lang: url_lang() }, "{t.overview}" }
                    }
                    Link { to: Route::ReposPage { lang: url_lang() }, "{t.nav_repos}" }
                    Link { to: Route::SkillsPage { lang: url_lang() }, "{t.nav_skills}" }
                    Link { to: Route::IndexPage { lang: url_lang() }, "{t.nav_index}" }
                    Link { to: Route::DownloadPage { lang: url_lang() }, "{t.nav_download}" }
                    Link { to: Route::DocsPage { lang: url_lang(), path: vec![] }, "Docs" }
                    if is_admin && *admin_mode.read() {
                        Link { to: Route::AdminUsersPage { lang: url_lang() }, "{t.users}" }
                        Link { to: Route::AdminIndexRulesPage { lang: url_lang() }, "{t.index_rules}" }
                    }
                    a {
                        class: "github-link",
                        href: CLIENT_REPO_URL,
                        target: "_blank",
                        rel: "noreferrer",
                        title: "GitHub",
                        dangerous_inner_html: r#"<svg viewBox="0 0 16 16" width="20" height="20" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>"#,
                    }
                }
                if is_admin {
                    button {
                        class: if *admin_mode.read() { "admin-toggle active" } else { "admin-toggle" },
                        onclick: move |_| {
                            let next = !*admin_mode.read();
                            admin_mode.set(next);
                            save_storage(ADMIN_MODE_STORAGE_KEY, if next { "true" } else { "false" });
                        },
                        crate::icons::IconSettings { size: 14 }
                        "Admin"
                    }
                }
                div { class: "auth-box",
                    {
                        let current_lang = *i18n_ctx.lang.read();
                        let current_lang_code = current_lang.code();
                        rsx! {
                            select {
                                class: "lang-toggle",
                                value: "{current_lang_code}",
                                onchange: move |evt: Event<FormData>| {
                                    let current_path = web_sys::window()
                                        .and_then(|w| w.location().pathname().ok())
                                        .unwrap_or_default();
                                    let new_lang = evt.value();
                                    let new_path = if let Some(rest) = current_path.strip_prefix("/en") {
                                        format!("/{new_lang}{rest}")
                                    } else if let Some(rest) = current_path.strip_prefix("/zh") {
                                        format!("/{new_lang}{rest}")
                                    } else {
                                        format!("/{new_lang}{current_path}")
                                    };
                                    save_storage(LANG_STORAGE_KEY, &new_lang);
                                    if let Some(w) = web_sys::window() {
                                        let _ = w.location().set_href(&new_path);
                                    }
                                },
                                option { value: "en", selected: current_lang_code == "en", "English" }
                                option { value: "zh", selected: current_lang_code == "zh", "中文" }
                            }
                        }
                    }
                    if !auth_notice_value.is_empty() {
                        p { class: "notice", "{auth_notice_value}" }
                    }
                    match &auth_state {
                        AuthState::Loading => rsx! {
                            span { class: "identity", "{t.checking}" }
                        },
                        AuthState::Anonymous => rsx! {
                            if !saved_accounts.is_empty() {
                                div { class: "saved-session-strip",
                                    span { class: "identity", "{t.saved_accounts}" }
                                    for session in saved_accounts.iter().cloned() {
                                        {
                                            let session_token = session.token.clone();
                                            let session_handle = session.handle.clone();
                                            let session_label = session
                                                .display_name
                                                .clone()
                                                .unwrap_or_else(|| format!("@{}", session_handle));
                                            let session_avatar = session.avatar_url.clone();
                                            rsx! {
                                                button {
                                                    class: "saved-session-chip",
                                                    onclick: move |_| {
                                                        store_auth_token(&session_token);
                                                        saved_sessions.set(mark_auth_session_used(&session_token));
                                                        token_signal.set(session_token.clone());
                                                        auth_notice_signal.set(String::new());
                                                        whoami.restart();
                                                    },
                                                    if let Some(url) = session_avatar {
                                                        img { src: "{url}", alt: "@{session_handle}", class: "user-avatar" }
                                                    } else {
                                                        span { class: "user-avatar-placeholder", "{session_handle.chars().next().unwrap_or('?')}" }
                                                    }
                                                    span { "{session_label}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            a {
                                class: "primary",
                                href: "{login_href}",
                                if saved_accounts.is_empty() { "{t.sign_in_github}" } else { "{t.add_account}" }
                            }
                        },
                        AuthState::Error(_error) => rsx! {
                            a { class: "secondary", href: "{login_href}", "{t.try_again}" }
                        },
                        AuthState::LoggedIn { handle, display_name, role: _, avatar_url } => {
                            let current_account_label = display_name
                                .clone()
                                .unwrap_or_else(|| format!("@{}", handle));
                            rsx! {
                                div { class: "user-menu",
                                button {
                                    class: "user-menu-trigger",
                                    onclick: move |_| {
                                        let current = *dropdown_open.read();
                                        dropdown_open.set(!current);
                                    },
                                    if let Some(url) = avatar_url {
                                        img { src: "{url}", alt: "@{handle}", class: "user-avatar" }
                                    } else {
                                        span { class: "user-avatar-placeholder", "{handle.chars().next().unwrap_or('?')}" }
                                    }
                                    span { class: "user-handle", "@{handle}" }
                                }
                                if *dropdown_open.read() {
                                    div { class: "dropdown-menu",
                                        p { class: "dropdown-label", "{current_account_label}" }
                                        Link {
                                            to: Route::UserPage { lang: url_lang(), handle: handle.clone() },
                                            class: "dropdown-item",
                                            onclick: move |_| dropdown_open.set(false),
                                            "{t.profile}"
                                        }
                                        if saved_accounts.iter().any(|session| session.token != token_value) {
                                            div { class: "dropdown-divider" }
                                            p { class: "dropdown-label", "{t.switch_account}" }
                                            for session in saved_accounts.iter().filter(|session| session.token != token_value).cloned() {
                                                {
                                                    let session_token = session.token.clone();
                                                    let session_handle = session.handle.clone();
                                                    let session_avatar = session.avatar_url.clone();
                                                    let session_label = session
                                                        .display_name
                                                        .clone()
                                                        .unwrap_or_else(|| format!("@{}", session_handle));
                                                    rsx! {
                                                        button {
                                                            class: "dropdown-item dropdown-account",
                                                            onclick: move |_| {
                                                                dropdown_open.set(false);
                                                                store_auth_token(&session_token);
                                                                saved_sessions.set(mark_auth_session_used(&session_token));
                                                                token_signal.set(session_token.clone());
                                                                auth_notice_signal.set(String::new());
                                                                whoami.restart();
                                                            },
                                                            if let Some(url) = session_avatar {
                                                                img { src: "{url}", alt: "@{session_handle}", class: "user-avatar" }
                                                            } else {
                                                                span { class: "user-avatar-placeholder", "{session_handle.chars().next().unwrap_or('?')}" }
                                                            }
                                                            span { "{session_label}" }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        div { class: "dropdown-divider" }
                                        a { class: "dropdown-item", href: "{login_href}", "{t.add_account}" }
                                        button {
                                            class: "dropdown-item",
                                            onclick: move |_| {
                                                dropdown_open.set(false);
                                                saved_sessions.set(remove_auth_session(&token_value));
                                                clear_stored_auth_token();
                                                token_signal.set(String::new());
                                                auth_notice_signal.set(String::new());
                                                whoami.restart();
                                            },
                                            "{t.log_out}"
                                        }
                                    }
                                }
                            }
                            }
                        },
                    }
                }
            }
            main { class: "page",
                Outlet::<Route> {}
            }
            footer { class: "site-footer",
                p { "\u{00A9} 2026 {t.brand_name} (Savhub). All rights reserved." }
            }
        }
    }
}

pub(crate) fn render_flock_cards(resource: &Resource<Result<PagedResponse<FlockSummary>, String>>) -> Element {
    let t = use_context::<I18nContext>().t();
    match &*resource.read_unchecked() {
        Some(Ok(payload)) => rsx! {
            div { class: "card-grid",
                for flock in payload.items.iter().cloned() {
                    FlockCard { flock }
                }
            }
        },
        Some(Err(error)) => rsx! { p { class: "error", "{error}" } },
        None => rsx! { p { "{t.loading}" } },
    }
}

pub(crate) fn render_source_label(source: &Option<CatalogSource>, t: &T) -> String {
    match source {
        Some(CatalogSource::Registry { .. }) => t.registry_source.to_string(),
        None => String::new(),
    }
}

fn security_badge_modifier(status: &SecurityStatus) -> &'static str {
    match status {
        SecurityStatus::Verified => "sb-verified",
        SecurityStatus::Checked => "sb-checked",
        SecurityStatus::Scanning => "sb-scanning",
        SecurityStatus::Suspicious => "sb-suspicious",
        SecurityStatus::Malicious => "sb-malicious",
        SecurityStatus::Unscanned => "sb-unscanned",
    }
}

fn security_badge_value<'a>(status: &SecurityStatus, t: &'a T) -> &'a str {
    match status {
        SecurityStatus::Verified => t.security_verified,
        SecurityStatus::Checked => t.security_checked,
        SecurityStatus::Scanning => t.security_scanning,
        SecurityStatus::Suspicious => t.security_suspicious,
        SecurityStatus::Malicious => t.security_malicious,
        SecurityStatus::Unscanned => t.security_unscanned,
    }
}

pub(crate) fn render_security_badge(status: &SecurityStatus, t: &T) -> Element {
    let modifier = security_badge_modifier(status);
    let value = security_badge_value(status, t);
    rsx! {
        span { class: "security-badge {modifier}",
            span { class: "sb-icon",
                if matches!(status, SecurityStatus::Verified | SecurityStatus::Checked) {
                    crate::icons::IconShieldCheck { size: 12, color: "#fff" }
                } else {
                    crate::icons::IconShield { size: 12, color: "#fff" }
                }
            }
            span { class: "sb-value", "{value}" }
        }
    }
}

fn index_steps(t: &T) -> Vec<(&str, i32)> {
    vec![
        (t.index_queued, 0),
        (t.index_cloning, 10),
        (t.index_scanning, 30),
        (t.index_categorizing, 50),
        (t.index_persisting, 70),
        (t.index_finalizing, 95),
        (t.index_done, 100),
    ]
}

pub(crate) fn render_ws_index_progress(
    evt: &IndexProgressEvent,
    t: &T,
    git_url: Option<&str>,
    git_ref: Option<&str>,
) -> Element {
    let pct = evt.progress_pct;
    let status_class = match evt.status.as_str() {
        "completed" => "pill badge-verified",
        "failed" => "pill badge-rejected",
        "running" => "pill badge-scanning",
        _ => "pill",
    };

    // Read accumulated message log for this job
    let ws = use_context::<WsIndexEvents>();
    let job_key = evt.job_id.to_string();
    let log = ws.message_log.read();
    let messages: Vec<String> = log.get(&job_key).cloned().unwrap_or_default();

    rsx! {
        div { class: "scan-job-card",
            div { class: "scan-job-head",
                span { class: "{status_class}", "{evt.status}" }
                code { "{evt.job_id}" }
                if evt.status == "failed" {
                    if let Some(url) = git_url {
                        { render_reindex_button(t, url, git_ref.unwrap_or("main")) }
                    }
                }
            }
            div { class: "progress-bar-container",
                div {
                    class: "progress-bar-fill",
                    style: "width: {pct}%",
                }
            }
            div { class: "scan-steps",
                for (label, threshold) in index_steps(t) {
                    {
                        let step_class = if pct > threshold {
                            "scan-step done"
                        } else if pct == threshold {
                            "scan-step active"
                        } else {
                            "scan-step"
                        };
                        let label = label.to_string();
                        rsx! {
                            div { class: "{step_class}",
                                span { class: "scan-step-dot" }
                                span { class: "scan-step-label", "{label}" }
                            }
                        }
                    }
                }
            }
            if !messages.is_empty() {
                div { class: "scan-log-box",
                    for msg in messages.iter() {
                        p { class: "scan-log-line", "{msg}" }
                    }
                }
            }
            if let Some(err) = evt.error_message.as_ref() {
                { let msg = friendly_error(err, t); rsx! { p { class: "error", "{msg}" } } }
            }
        }
    }
}

pub(crate) fn render_copy_sign(repo_url: &str, path: &str) -> Element {
    let display = format!("{}/{}", strip_url_scheme_keep_git(repo_url), path);
    let copy_json = format!("{{\"repo\":\"{}\",\"path\":\"{}\"}}", repo_url, path);
    let mut copied = use_signal(|| false);
    rsx! {
        span {
            class: "copy-sign",
            title: "Click to copy",
            onclick: move |evt: Event<MouseData>| {
                evt.stop_propagation();
                let v = copy_json.clone();
                copied.set(true);
                spawn(async move {
                    let _ = wasm_bindgen_futures::JsFuture::from(
                        web_sys::window()
                            .unwrap()
                            .navigator()
                            .clipboard()
                            .write_text(&v),
                    )
                    .await;
                    gloo_timers::future::TimeoutFuture::new(1500).await;
                    copied.set(false);
                });
            },
            span { class: "copy-sign-value", "{display}" }
            span { class: "copy-sign-btn",
                if *copied.read() {
                    crate::icons::IconCheck { size: 14, color: "#4ade80" }
                } else {
                    crate::icons::IconClipboard { size: 14 }
                }
            }
        }
    }
}

/// Compact copy button — just the icon, no sign text. For use in cards.
pub(crate) fn render_copy_icon(repo_url: &str, path: &str) -> Element {
    let display = format!("{}/{}", strip_url_scheme_keep_git(repo_url), path);
    let copy_json = format!("{{\"repo\":\"{}\",\"path\":\"{}\"}}", repo_url, path);
    let mut copied = use_signal(|| false);
    rsx! {
        span {
            class: "copy-icon-btn",
            title: "{display}",
            onclick: move |evt: Event<MouseData>| {
                evt.stop_propagation();
                let v = copy_json.clone();
                copied.set(true);
                spawn(async move {
                    let _ = wasm_bindgen_futures::JsFuture::from(
                        web_sys::window()
                            .unwrap()
                            .navigator()
                            .clipboard()
                            .write_text(&v),
                    )
                    .await;
                    gloo_timers::future::TimeoutFuture::new(1500).await;
                    copied.set(false);
                });
            },
            if *copied.read() {
                crate::icons::IconCheck { size: 14, color: "#4ade80" }
            } else {
                crate::icons::IconClipboard { size: 14 }
            }
        }
    }
}

fn render_reindex_button(t: &T, git_url: &str, git_ref: &str) -> Element {
    let api = use_context::<ApiContext>();
    let toast = use_context::<ToastContext>();
    let mut reindex_loading = use_signal(|| false);
    let mut reindex_done = use_signal(|| false);
    let url = git_url.to_string();
    let gref = git_ref.to_string();
    let reindex_label = t.reindex.to_string();
    let submitted_label = t.reindex_submitted.to_string();
    rsx! {
        if *reindex_done.read() {
            span { class: "btn-sm", "{submitted_label}" }
        } else {
            button {
                class: "btn-sm",
                disabled: *reindex_loading.read(),
                onclick: move |_| {
                    if *reindex_loading.read() { return; }
                    reindex_loading.set(true);
                    let token = api.token.read().clone();
                    let url = url.clone();
                    let gref = gref.clone();
                    spawn(async move {
                        let body = SubmitIndexRequest {
                            git_url: url,
                            git_ref: gref,
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
                            Ok(_) => reindex_done.set(true),
                            Err(e) => toast.error(e),
                        }
                        reindex_loading.set(false);
                    });
                },
                if *reindex_loading.read() {
                    span { class: "spinner" }
                }
                "{reindex_label}"
            }
        }
    }
}

pub(crate) fn render_index_job_detail(job: &IndexJobDto, t: &T) -> Element {
    let status_class = match job.status {
        IndexJobStatus::Completed => "pill badge-verified",
        IndexJobStatus::Failed => "pill badge-rejected",
        IndexJobStatus::Running => "pill badge-scanning",
        IndexJobStatus::Superseded => "pill badge-rejected",
        IndexJobStatus::Pending => "pill",
    };
    let status_label = format!("{:?}", job.status);
    let pct = job.progress_pct;

    // Read accumulated WS message log for this job (if any)
    let ws = use_context::<WsIndexEvents>();
    let job_key = job.id.to_string();
    let log = ws.message_log.read();
    let ws_messages: Vec<String> = log.get(&job_key).cloned().unwrap_or_default();

    rsx! {
        div { class: "scan-job-card",
            div { class: "scan-job-head",
                span { class: "{status_class}", "{status_label}" }
                code { "{job.git_url}" }
                if !job.git_ref.is_empty() {
                    span { class: "pill", "{job.git_ref}" }
                }
                if job.status == IndexJobStatus::Failed {
                    { render_reindex_button(t, &job.git_url, &job.git_ref) }
                }
            }
            div { class: "progress-bar-container",
                div {
                    class: "progress-bar-fill",
                    style: "width: {pct}%",
                }
            }
            div { class: "scan-steps",
                for (label, threshold) in index_steps(t) {
                    {
                        let step_class = if pct > threshold {
                            "scan-step done"
                        } else if pct == threshold {
                            "scan-step active"
                        } else {
                            "scan-step"
                        };
                        let label = label.to_string();
                        rsx! {
                            div { class: "{step_class}",
                                span { class: "scan-step-dot" }
                                span { class: "scan-step-label", "{label}" }
                            }
                        }
                    }
                }
            }
            if !ws_messages.is_empty() {
                div { class: "scan-log-box",
                    for msg in ws_messages.iter() {
                        p { class: "scan-log-line", "{msg}" }
                    }
                }
            } else if !job.progress_message.is_empty() {
                div { class: "scan-log-box",
                    p { class: "scan-log-line", "{job.progress_message}" }
                }
            }
            if let Some(err) = job.error_message.as_ref() {
                { let msg = friendly_error(err, t); rsx! { p { class: "error", "{msg}" } } }
            }
        }
    }
}

pub(crate) fn token_option(token: &str) -> Option<&str> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Format a UTC datetime as a local datetime string (YYYY-MM-DD HH:MM).
pub(crate) fn format_local_datetime(dt: chrono::DateTime<chrono::Utc>) -> String {
    let local = dt.with_timezone(&chrono::Local);
    local.format("%Y-%m-%d %H:%M").to_string()
}

/// Format a UTC datetime as a local short datetime string (MM-DD HH:MM).
pub(crate) fn format_local_datetime_short(dt: chrono::DateTime<chrono::Utc>) -> String {
    let local = dt.with_timezone(&chrono::Local);
    local.format("%m-%d %H:%M").to_string()
}

/// Format a UTC datetime as a local date string (YYYY-MM-DD).
pub(crate) fn format_local_date(dt: chrono::DateTime<chrono::Utc>) -> String {
    let local = dt.with_timezone(&chrono::Local);
    local.format("%Y-%m-%d").to_string()
}

pub(crate) fn relative_time_i18n(dt: chrono::DateTime<chrono::Utc>, t: &T) -> String {
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(dt);
    let secs = diff.num_seconds();
    if secs < 60 {
        t.just_now.to_string()
    } else if secs < 3600 {
        let m = diff.num_minutes();
        format!("{m} {}", t.time_m_ago)
    } else if secs < 86400 {
        let h = diff.num_hours();
        format!("{h} {}", t.time_h_ago)
    } else if secs < 2_592_000 {
        let d = diff.num_days();
        format!("{d} {}", t.time_d_ago)
    } else if secs < 31_536_000 {
        let m = diff.num_days() / 30;
        format!("{m} {}", t.time_mo_ago)
    } else {
        let y = diff.num_days() / 365;
        format!("{y} {}", t.time_y_ago)
    }
}

pub(crate) fn friendly_error(error: &str, t: &T) -> String {
    if error.contains("401") || error.contains("Unauthorized") {
        t.err_session_expired.to_string()
    } else if error.contains("403") || error.contains("Forbidden") {
        t.err_no_permission.to_string()
    } else if error.contains("404") || error.contains("Not Found") {
        t.err_not_found.to_string()
    } else if error.contains("timeout") || error.contains("Timeout") {
        t.err_timeout.to_string()
    } else if error.contains("NetworkError") || error.contains("fetch") {
        t.err_network.to_string()
    } else if error.contains("500")
        || error.contains("502")
        || error.contains("503")
        || error.contains("ConnectionRefused")
        || error.contains("Proxy connection failed")
    {
        t.err_server.to_string()
    } else if error.starts_with(|c: char| c.is_ascii_uppercase())
        && error.len() < 200
        && !error.contains("Error: ")
        && !error.contains("at ")
    {
        // Looks like a server-supplied human message — surface it directly.
        error.to_string()
    } else {
        t.err_unexpected.to_string()
    }
}

fn initial_auth_state() -> &'static InitialAuthState {
    static INITIAL_AUTH_STATE: OnceLock<InitialAuthState> = OnceLock::new();
    INITIAL_AUTH_STATE.get_or_init(load_initial_auth_state)
}

#[cfg(target_arch = "wasm32")]
fn load_initial_auth_state() -> InitialAuthState {
    let token_from_storage = load_stored_auth_token();
    let mut token = token_from_storage;
    let mut notice = String::new();

    // Check hash fragment (non-loopback redirects)
    let hash = current_hash();
    if !hash.is_empty() {
        let params = parse_hash_params(&hash);
        if let Some(value) = params.get("auth_token") {
            token = value.clone();
            store_auth_token(value);
            notice.clear();
        }
        if let Some(value) = params.get("auth_error") {
            notice = value.clone();
        }
        clear_location_hash();
    }

    // Check query parameters (loopback redirects)
    let query = current_query();
    if !query.is_empty() {
        let params = parse_hash_params(&query);
        if let Some(value) = params.get("auth_token") {
            token = value.clone();
            store_auth_token(value);
            notice.clear();
            clear_location_query();
        }
        if let Some(value) = params.get("auth_error") {
            notice = value.clone();
            clear_location_query();
        }
    }

    InitialAuthState { token, notice }
}

#[cfg(not(target_arch = "wasm32"))]
fn load_initial_auth_state() -> InitialAuthState {
    InitialAuthState::default()
}

pub(crate) fn github_login_url(api_base: &str) -> String {
    let base = api_base.trim_end_matches('/');
    let endpoint = format!("{base}/auth/github/start");
    let mut url = match reqwest::Url::parse(&endpoint) {
        Ok(url) => url,
        Err(_) => {
            return endpoint;
        }
    };

    if let Some(return_to) = current_page_url() {
        url.query_pairs_mut().append_pair("return_to", &return_to);
    }

    url.to_string()
}

#[cfg(target_arch = "wasm32")]
fn load_stored_auth_token() -> String {
    browser_storage()
        .and_then(|storage| storage.get_item(AUTH_TOKEN_STORAGE_KEY).ok().flatten())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn load_stored_auth_token() -> String {
    String::new()
}

#[cfg(target_arch = "wasm32")]
fn store_auth_token(token: &str) {
    if let Some(storage) = browser_storage() {
        let _ = storage.set_item(AUTH_TOKEN_STORAGE_KEY, token);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn store_auth_token(_token: &str) {}

#[cfg(target_arch = "wasm32")]
fn clear_stored_auth_token() {
    if let Some(storage) = browser_storage() {
        let _ = storage.remove_item(AUTH_TOKEN_STORAGE_KEY);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_stored_auth_token() {}

#[cfg(target_arch = "wasm32")]
fn load_auth_sessions() -> Vec<StoredAuthSession> {
    load_storage(AUTH_SESSIONS_STORAGE_KEY)
        .and_then(|raw| serde_json::from_str::<Vec<StoredAuthSession>>(&raw).ok())
        .map(normalize_auth_sessions)
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn load_auth_sessions() -> Vec<StoredAuthSession> {
    Vec::new()
}

fn save_auth_sessions(sessions: &[StoredAuthSession]) {
    let normalized = normalize_auth_sessions(sessions.to_vec());
    if normalized.is_empty() {
        remove_storage(AUTH_SESSIONS_STORAGE_KEY);
        return;
    }
    if let Ok(raw) = serde_json::to_string(&normalized) {
        save_storage(AUTH_SESSIONS_STORAGE_KEY, &raw);
    }
}

fn normalize_auth_sessions(mut sessions: Vec<StoredAuthSession>) -> Vec<StoredAuthSession> {
    sessions
        .retain(|session| !session.token.trim().is_empty() && !session.handle.trim().is_empty());
    sessions.sort_by(|left, right| right.last_used_at.cmp(&left.last_used_at));
    sessions
}

fn upsert_auth_session(token: &str, user: &savhub_shared::UserSummary) -> Vec<StoredAuthSession> {
    let mut sessions = load_auth_sessions();
    let now = chrono::Utc::now().timestamp();
    if let Some(existing) = sessions.iter_mut().find(|session| session.token == token) {
        existing.handle = user.handle.clone();
        existing.display_name = user.display_name.clone();
        existing.avatar_url = user.avatar_url.clone();
        existing.role = user.role;
        existing.last_used_at = now;
    } else {
        sessions.push(StoredAuthSession {
            token: token.to_string(),
            handle: user.handle.clone(),
            display_name: user.display_name.clone(),
            avatar_url: user.avatar_url.clone(),
            role: user.role,
            last_used_at: now,
        });
    }
    save_auth_sessions(&sessions);
    load_auth_sessions()
}

fn mark_auth_session_used(token: &str) -> Vec<StoredAuthSession> {
    let mut sessions = load_auth_sessions();
    if let Some(existing) = sessions.iter_mut().find(|session| session.token == token) {
        existing.last_used_at = chrono::Utc::now().timestamp();
    }
    save_auth_sessions(&sessions);
    load_auth_sessions()
}

fn remove_auth_session(token: &str) -> Vec<StoredAuthSession> {
    let mut sessions = load_auth_sessions();
    sessions.retain(|session| session.token != token);
    save_auth_sessions(&sessions);
    load_auth_sessions()
}

pub(crate) fn short_repo_name(url: &str) -> &str {
    let url = url.trim_end_matches('/').trim_end_matches(".git");
    url.rsplit('/').next().unwrap_or(url)
}





#[component]
pub(crate) fn MetadataPanel(metadata: BundleMetadata) -> Element {
    let t = use_context::<I18nContext>().t();
    let links = metadata
        .links
        .iter()
        .map(|(label, value)| (label.clone(), value.clone()))
        .collect::<Vec<_>>();
    let installs = metadata.runtime.install.clone();
    let bins = metadata.runtime.bins.clone();
    let env = metadata.runtime.env.clone();
    let keywords = metadata.discovery.keywords.clone();
    let categories = metadata.discovery.categories.clone();
    let git_ref = metadata.source.git.as_ref().map(|git| {
        (
            format!("{:?}", git.reference.kind).to_lowercase(),
            git.reference.value.clone(),
            git.url.clone(),
        )
    });

    rsx! {
        div { class: "panel",
            h2 { "{t.metadata}" }
            div { class: "meta-stack",
                p { class: "meta-pair",
                    strong { "{t.author}" }
                    span { "{metadata.author.name}" }
                }
                p { class: "meta-pair",
                    strong { "{t.license}" }
                    span { "{metadata.package.license}" }
                }
                p { class: "meta-pair",
                    strong { "{t.source}" }
                    span {
                        match metadata.source.kind {
                            BundleSourceKind::Bundle => {t.bundled},
                            BundleSourceKind::Git => {t.git},
                            _ => {t.bundled},
                        }
                    }
                }
                p { class: "meta-pair",
                    strong { "{t.subdir}" }
                    code { "{metadata.source.subdir}" }
                }
                if let Some(icon) = metadata.package.icon.clone() {
                    p { class: "meta-pair",
                        strong { "{t.icon}" }
                        span { "{icon}" }
                    }
                }
                if let Some((git_ref_kind, git_ref_value, git_url)) = git_ref {
                    p { class: "meta-pair wide",
                        strong { "{t.git_url}" }
                        a { href: "{git_url}", target: "_blank", rel: "noreferrer", "{git_url}" }
                    }
                    p { class: "meta-pair",
                        strong { "{t.git_ref}" }
                        span { "{git_ref_kind} · {git_ref_value}" }
                    }
                }
                if !keywords.is_empty() {
                    p { class: "meta-pair wide",
                        strong { "{t.keywords}" }
                        code { "{keywords.join(\", \")}" }
                    }
                }
                if !categories.is_empty() {
                    p { class: "meta-pair wide",
                        strong { "{t.categories}" }
                        code { "{categories.join(\", \")}" }
                    }
                }
                if !bins.is_empty() {
                    p { class: "meta-pair wide",
                        strong { "{t.bins}" }
                        code { "{bins.join(\", \")}" }
                    }
                }
                if !env.is_empty() {
                    p { class: "meta-pair wide",
                        strong { "{t.env}" }
                        code { "{env.join(\", \")}" }
                    }
                }
                if !installs.is_empty() {
                    div {
                        strong { "{t.install}" }
                        ul { class: "dense-list compact-list",
                            for install in installs {
                                li {
                                    strong { "{install.label.clone().unwrap_or_else(|| install.kind.clone())}" }
                                    if let Some(package) = install.package.clone() {
                                        span { " · {package}" }
                                    }
                                    if let Some(command) = install.command.clone() {
                                        code { " {command}" }
                                    }
                                }
                            }
                        }
                    }
                }
                if !links.is_empty() {
                    div {
                        strong { "{t.links}" }
                        ul { class: "dense-list compact-list",
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
    }
}

#[component]
pub(crate) fn CommentsList(
    comments: Vec<CommentDto>,
    empty_label: &'static str,
    delete_label: &'static str,
    on_delete: EventHandler<uuid::Uuid>,
) -> Element {
    let t = use_context::<I18nContext>().t();
    rsx! {
        if comments.is_empty() {
            p { class: "muted comment-empty", "{empty_label}" }
        } else {
            ul { class: "comment-list",
                for comment in comments {
                    {
                        let comment_id = comment.id;
                        let created_at = relative_time_i18n(comment.created_at, t);
                        rsx! {
                            li { class: "comment",
                                div { class: "comment-head",
                                    div { class: "comment-meta",
                                        span { class: "comment-author", "@{comment.user.handle}" }
                                        span { class: "muted", "{created_at}" }
                                    }
                                    if comment.can_delete {
                                        button {
                                            class: "comment-delete",
                                            onclick: move |_| on_delete.call(comment_id),
                                            "{delete_label}"
                                        }
                                    }
                                }
                                p { class: "comment-body", "{comment.body}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

