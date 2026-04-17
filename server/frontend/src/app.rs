#![cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, unused_imports, unused_mut)
)]

use std::sync::OnceLock;

use dioxus::prelude::*;
use dioxus_router::{Link, Outlet, Routable, Router};
use savhub_shared::{
    AdminActionResponse, BrowseHistoryItem, BundleMetadata, BundleSourceKind, CatalogSource,
    CommentDto, CreateCommentRequest, CreateIndexRuleRequest, DeleteResponse, DocPageResponse,
    FlockDetailResponse, FlockSummary, IndexJobDto, IndexJobListResponse, IndexJobStatus,
    IndexRuleDto, IndexRuleListResponse, ManagementSummaryResponse, PagedResponse,
    RecordViewRequest, RepoDetailResponse, RepoSummary, RoleUpdateResponse, SecurityStatus,
    SetUserRoleRequest, SkillDetailResponse, SkillListItem, SubmitIndexRequest,
    SubmitIndexResponse, ToggleStarResponse, UpdateIndexRuleRequest, UpdateSecurityStatusRequest,
    UserListResponse, UserProfileResponse, UserRole, WhoAmIResponse,
};

use crate::api;
use crate::i18n::{self, Lang, T};

const SAVHUB_LOGO: Asset = asset!("/assets/savhub.svg");
const SAVHUB_FAVICON: Asset = asset!("/assets/savhub.ico");
const BAMBOO_DECOR: Asset = asset!("/assets/bamboo_decor.svg");
const AUTH_TOKEN_STORAGE_KEY: &str = "savhub.auth_token";
const AUTH_SESSIONS_STORAGE_KEY: &str = "savhub.auth_sessions";
const LANG_STORAGE_KEY: &str = "savhub.lang";
const SCROLL_PAGE_SIZE: i64 = 20;

/// Derive a human-readable repo slug from git_url (e.g. `https://github.com/org/repo.git` → `github.com/org/repo`).
fn derive_repo_slug(git_url: &str) -> String {
    strip_url_scheme(git_url).to_string()
}

/// Strip `https://` / `http://` prefix and `.git` suffix for display.
fn strip_url_scheme(url: &str) -> &str {
    let url = url.trim().trim_end_matches('/');
    let url = url.trim_end_matches(".git");
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}

/// Strip only `https://` / `http://` prefix, keeping `.git` suffix intact.
fn strip_url_scheme_keep_git(url: &str) -> &str {
    let url = url.trim().trim_end_matches('/');
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}
const ADMIN_MODE_STORAGE_KEY: &str = "savhub.admin_mode";
const CLIENT_REPO_URL: &str = "https://github.com/savhub-ai/savhub";
const CLIENT_RELEASES_URL: &str = "https://github.com/savhub-ai/savhub/releases";
const CLIENT_LATEST_RELEASE_URL: &str = "https://github.com/savhub-ai/savhub/releases/latest";
const INSTALL_SH_URL: &str =
    "https://raw.githubusercontent.com/savhub-ai/savhub/main/scripts/install.sh";
const INSTALL_PS1_URL: &str =
    "https://raw.githubusercontent.com/savhub-ai/savhub/main/scripts/install.ps1";

#[derive(Clone, Copy)]
struct ApiContext {
    api_base: &'static str,
    token: Signal<String>,
    auth_notice: Signal<String>,
}

#[derive(Clone, Copy)]
struct I18nContext {
    lang: Signal<Lang>,
}

impl I18nContext {
    fn t(&self) -> &'static T {
        i18n::translations(*self.lang.read())
    }
}

/// Get the current language code from I18nContext for constructing Route links.
fn url_lang() -> String {
    use_context::<I18nContext>().lang.read().code().to_string()
}

/// Build a `Route::RepoPage` from a combined slug like `"github.com/owner/name"`.
fn repo_route(slug: &str) -> Route {
    let parts: Vec<&str> = slug.splitn(3, '/').collect();
    Route::RepoPage {
        lang: url_lang(),
        domain: parts.first().unwrap_or(&"").to_string(),
        owner: parts.get(1).unwrap_or(&"").to_string(),
        name: parts.get(2).unwrap_or(&"").to_string(),
    }
}

// -- Toast notification system --

#[derive(Clone, Debug, PartialEq)]
enum ToastKind {
    Error,
    Success,
    Info,
}

#[derive(Clone, Debug)]
struct ToastItem {
    id: u64,
    kind: ToastKind,
    message: String,
}

#[derive(Clone, Copy)]
struct ToastContext {
    items: Signal<Vec<ToastItem>>,
    counter: Signal<u64>,
}

impl ToastContext {
    fn push(mut self, kind: ToastKind, message: impl Into<String>) {
        let id = {
            let v = *self.counter.read() + 1;
            self.counter.set(v);
            v
        };
        let item = ToastItem {
            id,
            kind,
            message: message.into(),
        };
        self.items.write().push(item);
        // Auto-dismiss after 6 seconds
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(6_000).await;
            self.dismiss(id);
        });
    }

    fn error(self, message: impl Into<String>) {
        self.push(ToastKind::Error, message);
    }

    fn success(self, message: impl Into<String>) {
        self.push(ToastKind::Success, message);
    }

    fn dismiss(mut self, id: u64) {
        self.items.write().retain(|t| t.id != id);
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

/// Which admin tab is active, shared between topbar nav and ManagementPage.
#[derive(Clone, Copy)]
struct AdminTabCtx {
    tab: Signal<&'static str>,
}

/// Latest scan progress event from WebSocket, keyed by job_id.
#[derive(Clone, Copy)]
struct WsIndexEvents {
    events: Signal<std::collections::BTreeMap<String, IndexProgressEvent>>,
    /// Accumulated progress message log per job_id (distinct, ordered).
    message_log: Signal<std::collections::BTreeMap<String, Vec<String>>>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct IndexProgressEvent {
    pub job_id: uuid::Uuid,
    pub status: String,
    pub progress_pct: i32,
    pub progress_message: String,
    pub result_data: serde_json::Value,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct InitialAuthState {
    token: String,
    notice: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StoredAuthSession {
    token: String,
    handle: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
    role: UserRole,
    last_used_at: i64,
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
enum Route {
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

#[cfg(target_arch = "wasm32")]
thread_local! {
    static WS_HANDLE: std::cell::RefCell<Option<web_sys::WebSocket>> = std::cell::RefCell::new(None);
    /// Current reconnect backoff in milliseconds. Doubles on each failed
    /// attempt up to a 30s ceiling, resets to 1s on a successful open.
    static WS_BACKOFF_MS: std::cell::Cell<u32> = std::cell::Cell::new(1_000);
    /// Active subscriptions that must be re-sent after a reconnect.
    static WS_SUBSCRIPTIONS: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

#[cfg(target_arch = "wasm32")]
const WS_BACKOFF_MAX_MS: u32 = 30_000;

#[cfg(target_arch = "wasm32")]
fn start_ws_connection(ws_events: WsIndexEvents) {
    connect_ws(ws_events);
}

#[cfg(target_arch = "wasm32")]
fn connect_ws(mut ws_events: WsIndexEvents) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    let origin = current_origin().unwrap_or_default();
    let ws_url = origin.replacen("http", "ws", 1);
    let url = format!("{}/api/v1/ws", ws_url);

    let ws = match web_sys::WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(_) => {
            schedule_ws_reconnect(ws_events);
            return;
        }
    };

    WS_HANDLE.with(|cell| {
        *cell.borrow_mut() = Some(ws.clone());
    });

    // On open: reset backoff and replay any active subscriptions so the
    // server resumes streaming events for them after the reconnect.
    let onopen = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
        WS_BACKOFF_MS.with(|c| c.set(1_000));
        WS_HANDLE.with(|cell| {
            if let Some(ws) = cell.borrow().as_ref() {
                WS_SUBSCRIPTIONS.with(|subs| {
                    for id in subs.borrow().iter() {
                        let msg = format!(r#"{{"action":"subscribe","job_id":"{}"}}"#, id);
                        let _ = ws.send_with_str(&msg);
                    }
                });
            }
        });
    });
    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    onopen.forget();

    let onmessage =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if let Some(text) = event.data().as_string() {
                if let Ok(evt) = serde_json::from_str::<IndexProgressEvent>(&text) {
                    let key = evt.job_id.to_string();
                    if !evt.progress_message.is_empty() {
                        let msg = evt.progress_message.clone();
                        let mut log = ws_events.message_log.write();
                        let entry = log.entry(key.clone()).or_insert_with(Vec::new);
                        if entry.last().map_or(true, |last| *last != msg) {
                            entry.push(msg);
                        }
                    }
                    ws_events.events.write().insert(key, evt);
                }
            }
        });
    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    // On close / error: schedule a reconnect with exponential backoff.
    let events_for_close = ws_events;
    let onclose = Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |_| {
        WS_HANDLE.with(|cell| {
            *cell.borrow_mut() = None;
        });
        schedule_ws_reconnect(events_for_close);
    });
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    onclose.forget();
}

#[cfg(target_arch = "wasm32")]
fn schedule_ws_reconnect(ws_events: WsIndexEvents) {
    use wasm_bindgen::prelude::*;

    let delay = WS_BACKOFF_MS.with(|c| {
        let cur = c.get();
        c.set((cur.saturating_mul(2)).min(WS_BACKOFF_MAX_MS));
        cur
    });

    let cb = Closure::once_into_js(move || {
        connect_ws(ws_events);
    });
    if let Some(w) = web_sys::window() {
        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            delay as i32,
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn start_ws_connection(_ws_events: WsIndexEvents) {}

#[cfg(target_arch = "wasm32")]
fn ws_subscribe(job_id: &str) {
    // Track the subscription so we can replay it after a reconnect.
    WS_SUBSCRIPTIONS.with(|subs| {
        subs.borrow_mut().insert(job_id.to_string());
    });
    WS_HANDLE.with(|cell| {
        if let Some(ws) = cell.borrow().as_ref() {
            if ws.ready_state() == 1 {
                let msg = format!(r#"{{"action":"subscribe","job_id":"{}"}}"#, job_id);
                let _ = ws.send_with_str(&msg);
            }
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn ws_subscribe(_job_id: &str) {}

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

#[component]
fn Home(lang: String) -> Element {
    let _ = &lang;
    let api = use_context::<ApiContext>();
    let token = api.token.read().clone();
    let popular_token = token.clone();
    let recent_token = token.clone();
    let popular_flocks = use_resource(move || {
        let token = popular_token.clone();
        async move {
            api::get_json::<PagedResponse<FlockSummary>>(
                api.api_base,
                token_option(&token),
                "/flocks?limit=6&sort=stars",
            )
            .await
        }
    });
    let recent_flocks = use_resource(move || {
        let token = recent_token.clone();
        async move {
            api::get_json::<PagedResponse<FlockSummary>>(
                api.api_base,
                token_option(&token),
                "/flocks?limit=6&sort=updated",
            )
            .await
        }
    });
    let t = use_context::<I18nContext>().t();
    rsx! {
        document::Title { "{t.home_title}" }
        section { class: "hero",
            div { class: "hero-copy",
                p { class: "eyebrow", "{t.home_eyebrow}" }
                h1 { "{t.home_headline}" }
                p { class: "hero-text",
                    "{t.home_hero_text}"
                }
                div { class: "hero-actions",
                    Link { class: "primary", to: Route::SkillsPage { lang: url_lang() }, "{t.home_browse_skills}" }
                    Link { class: "secondary", to: Route::IndexPage { lang: url_lang() }, "{t.home_publish_bundle}" }
                }
            }
        }
        section { class: "section",
            div { class: "section-head",
                h2 { "{t.home_popular_skills}" }
                Link { to: Route::SkillsPage { lang: url_lang() }, "{t.see_all}" }
            }
            {render_flock_cards(&popular_flocks)}
        }
        section { class: "section",
            div { class: "section-head",
                h2 { "{t.home_recently_updated}" }
                Link { to: Route::SkillsPage { lang: url_lang() }, "{t.see_all}" }
            }
            {render_flock_cards(&recent_flocks)}
        }
    }
}

#[component]
fn SkillsPage(lang: String) -> Element {
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
fn SkillFlockList(
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
fn FlockListRow(flock: FlockSummary) -> Element {
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
fn SkillListRow(skill: SkillListItem) -> Element {
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

#[component]
fn ReposPage(lang: String) -> Element {
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
fn RepoListRow(repo: RepoSummary, #[props(default = false)] is_admin: bool) -> Element {
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

#[component]
fn InstallBlock(label: &'static str, command: String) -> Element {
    let cmd = command.clone();
    let mut copied = use_signal(|| false);
    rsx! {
        article { class: "card install-block",
            div { class: "install-block-head",
                h3 { "{label}" }
                button {
                    class: if copied() { "install-copy-btn copied" } else { "install-copy-btn" },
                    title: "Copy",
                    onclick: move |_| {
                        let js = format!(
                            "navigator.clipboard.writeText({})",
                            serde_json::to_string(&cmd).unwrap_or_default()
                        );
                        document::eval(&js);
                        copied.set(true);
                        let mut copied = copied;
                        spawn(async move {
                            gloo_timers::future::TimeoutFuture::new(2000).await;
                            copied.set(false);
                        });
                    },
                    if copied() {
                        crate::icons::IconCheck { size: 14, color: "#4ade80" }
                    } else {
                        crate::icons::IconClipboard { size: 14 }
                    }
                }
            }
            pre { class: "install-code",
                code { "{command}" }
            }
        }
    }
}

#[component]
fn DownloadPage(lang: String) -> Element {
    let _ = &lang;
    let t = use_context::<I18nContext>().t();
    rsx! {
        document::Title { "{t.download_title}" }

        // ── 1. Demo (asciinema) ──
        section { class: "section download-hero",
            h1 { "{t.download_heading}" }
            p { class: "muted", "{t.download_intro}" }
        }

        // ── 2. Quick install ──
        section { class: "section",
            h2 { "{t.download_quick_install}" }
            div { class: "install-stack",
                InstallBlock { label: t.download_install_linux_macos, command: format!("curl -fsSL {INSTALL_SH_URL} | bash") }
                InstallBlock { label: t.download_install_windows, command: format!("irm {INSTALL_PS1_URL} | iex") }
            }
        }

        // ── 3. Download ──
        section { class: "section",
            h2 { "{t.nav_download}" }
            p { class: "muted", "{t.download_package_note}" }
            div { class: "download-platform-grid",
                // Windows
                article { class: "card download-platform-card",
                    h3 { "{t.download_windows}" }
                    div { class: "download-card-links",
                        a { class: "primary", href: CLIENT_LATEST_RELEASE_URL, target: "_blank", rel: "noreferrer",
                            ".msi"
                        }
                        a { class: "dl-link", href: CLIENT_LATEST_RELEASE_URL, target: "_blank", rel: "noreferrer",
                            ".zip"
                        }
                    }
                }
                // macOS
                article { class: "card download-platform-card",
                    h3 { "{t.download_macos}" }
                    div { class: "download-card-links",
                        a { class: "primary", href: CLIENT_LATEST_RELEASE_URL, target: "_blank", rel: "noreferrer",
                            ".pkg"
                        }
                        a { class: "dl-link", href: CLIENT_LATEST_RELEASE_URL, target: "_blank", rel: "noreferrer",
                            ".tar.gz"
                        }
                    }
                }
                // Linux
                article { class: "card download-platform-card",
                    h3 { "{t.download_linux}" }
                    div { class: "download-card-links",
                        a { class: "primary", href: CLIENT_LATEST_RELEASE_URL, target: "_blank", rel: "noreferrer",
                            ".deb"
                        }
                        a { class: "dl-link", href: CLIENT_LATEST_RELEASE_URL, target: "_blank", rel: "noreferrer",
                            ".tar.gz"
                        }
                    }
                }
            }
            div { class: "download-footer",
                a { class: "secondary", href: CLIENT_RELEASES_URL, target: "_blank", rel: "noreferrer",
                    "{t.download_all_releases}"
                }
                a { class: "secondary", href: CLIENT_REPO_URL, target: "_blank", rel: "noreferrer",
                    "{t.download_source_repo}"
                }
            }
        }
    }
}

#[component]
fn RepoPage(lang: String, domain: String, owner: String, name: String) -> Element {
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

#[component]
fn FlockPage(lang: String, id: String) -> Element {
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

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    // If the path doesn't start with a valid lang prefix, try redirecting with one.
    #[cfg(target_arch = "wasm32")]
    {
        let first = segments.first().map(|s| s.as_str()).unwrap_or("");
        if first != "en" && first != "zh" {
            let saved_lang = load_storage(LANG_STORAGE_KEY)
                .filter(|s| s == "en" || s == "zh")
                .unwrap_or_else(|| "en".to_string());
            let path = format!("/{}", segments.join("/"));
            let new_url = format!("/{saved_lang}{path}");
            if let Some(w) = web_sys::window() {
                let _ = w.location().replace(&new_url);
            }
            return rsx! {};
        }
    }

    let joined = if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    };
    let t = use_context::<I18nContext>().t();
    rsx! {
        document::Title { "{t.not_found_title}" }
        section { class: "section",
            h2 { "{t.not_found}" }
            p { "No route matched " code { "{joined}" } "." }
        }
    }
}

fn render_skill_cards(
    resource: &Resource<Result<PagedResponse<SkillListItem>, String>>,
) -> Element {
    let t = use_context::<I18nContext>().t();
    match &*resource.read_unchecked() {
        Some(Ok(payload)) => rsx! {
            div { class: "card-grid",
                for skill in payload.items.iter().cloned() {
                    SkillCard { skill }
                }
            }
        },
        Some(Err(error)) => rsx! { p { class: "error", "{error}" } },
        None => rsx! { p { "{t.loading}" } },
    }
}

fn render_flock_cards(resource: &Resource<Result<PagedResponse<FlockSummary>, String>>) -> Element {
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

fn render_repo_cards(resource: &Resource<Result<PagedResponse<RepoSummary>, String>>) -> Element {
    let t = use_context::<I18nContext>().t();
    match &*resource.read_unchecked() {
        Some(Ok(payload)) => rsx! {
            div { class: "card-grid",
                for repo in payload.items.iter().cloned() {
                    RepoCard { repo }
                }
            }
        },
        Some(Err(error)) => rsx! { p { class: "error", "{error}" } },
        None => rsx! { p { "{t.loading}" } },
    }
}

#[component]
fn SkillCard(skill: SkillListItem) -> Element {
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
fn RepoCard(repo: savhub_shared::RepoSummary) -> Element {
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
fn FlockCard(flock: savhub_shared::FlockSummary) -> Element {
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

fn render_source_label(source: &Option<CatalogSource>, t: &T) -> String {
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

fn render_security_badge(status: &SecurityStatus, t: &T) -> Element {
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

fn render_ws_index_progress(
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

fn render_copy_sign(repo_url: &str, path: &str) -> Element {
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
fn render_copy_icon(repo_url: &str, path: &str) -> Element {
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

fn render_index_job_detail(job: &IndexJobDto, t: &T) -> Element {
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

fn token_option(token: &str) -> Option<&str> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Format a UTC datetime as a local datetime string (YYYY-MM-DD HH:MM).
fn format_local_datetime(dt: chrono::DateTime<chrono::Utc>) -> String {
    let local = dt.with_timezone(&chrono::Local);
    local.format("%Y-%m-%d %H:%M").to_string()
}

/// Format a UTC datetime as a local short datetime string (MM-DD HH:MM).
fn format_local_datetime_short(dt: chrono::DateTime<chrono::Utc>) -> String {
    let local = dt.with_timezone(&chrono::Local);
    local.format("%m-%d %H:%M").to_string()
}

/// Format a UTC datetime as a local date string (YYYY-MM-DD).
fn format_local_date(dt: chrono::DateTime<chrono::Utc>) -> String {
    let local = dt.with_timezone(&chrono::Local);
    local.format("%Y-%m-%d").to_string()
}

fn relative_time(dt: chrono::DateTime<chrono::Utc>) -> String {
    let t = i18n::translations(Lang::En);
    relative_time_i18n(dt, t)
}

fn relative_time_i18n(dt: chrono::DateTime<chrono::Utc>, t: &T) -> String {
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

fn friendly_error(error: &str, t: &T) -> String {
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
    } else {
        error.to_string()
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

fn github_login_url(api_base: &str) -> String {
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
fn current_origin() -> Option<String> {
    web_sys::window()?.location().origin().ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn current_origin() -> Option<String> {
    None
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
fn browser_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn load_storage(key: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        browser_storage()?
            .get_item(key)
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        None
    }
}

fn save_storage(key: &str, value: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = browser_storage() {
            let _ = storage.set_item(key, value);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (key, value);
    }
}

fn remove_storage(key: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = browser_storage() {
            let _ = storage.remove_item(key);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
    }
}

fn load_auth_sessions() -> Vec<StoredAuthSession> {
    load_storage(AUTH_SESSIONS_STORAGE_KEY)
        .and_then(|raw| serde_json::from_str::<Vec<StoredAuthSession>>(&raw).ok())
        .map(normalize_auth_sessions)
        .unwrap_or_default()
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

/// Check whether the window is scrolled near the bottom (within `threshold` pixels).
#[cfg(target_arch = "wasm32")]
fn near_window_bottom(threshold: f64) -> bool {
    web_sys::window()
        .and_then(|w| {
            let scroll_y = w.scroll_y().ok()?;
            let inner_h = w.inner_height().ok()?.as_f64()?;
            let doc = w.document()?;
            let el = doc.document_element()?;
            let scroll_h = el.scroll_height() as f64;
            Some(scroll_y + inner_h + threshold >= scroll_h)
        })
        .unwrap_or(false)
}

#[cfg(not(target_arch = "wasm32"))]
fn near_window_bottom(_threshold: f64) -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
fn current_query() -> String {
    web_sys::window()
        .and_then(|window| window.location().search().ok())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn current_query() -> String {
    String::new()
}

#[cfg(target_arch = "wasm32")]
fn clear_location_query() {
    if let Some(window) = web_sys::window() {
        if let Ok(pathname) = window.location().pathname() {
            let _ = window.history().ok().and_then(|history| {
                history
                    .replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&pathname))
                    .ok()
            });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_location_query() {}

/// Read a single query-string parameter from the current browser URL.
fn query_param(key: &str) -> Option<String> {
    let qs = current_query();
    let qs = qs.strip_prefix('?').unwrap_or(&qs);
    for pair in qs.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(v.to_string());
        }
    }
    None
}

/// Replace the browser URL query string (without navigation) to reflect current list state.
#[cfg(target_arch = "wasm32")]
fn set_location_query(params: &[(&str, &str)]) {
    if let Some(window) = web_sys::window() {
        if let Ok(pathname) = window.location().pathname() {
            let qs: Vec<String> = params
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            let url = if qs.is_empty() {
                pathname
            } else {
                format!("{pathname}?{}", qs.join("&"))
            };
            let _ = window.history().ok().and_then(|history| {
                history
                    .replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
                    .ok()
            });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn set_location_query(_params: &[(&str, &str)]) {}

#[cfg(target_arch = "wasm32")]
fn current_hash() -> String {
    web_sys::window()
        .and_then(|window| window.location().hash().ok())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn current_hash() -> String {
    String::new()
}

#[cfg(target_arch = "wasm32")]
fn current_page_url() -> Option<String> {
    let href = web_sys::window()?.location().href().ok()?;
    let mut url = reqwest::Url::parse(&href).ok()?;
    url.set_fragment(None);
    Some(url.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn current_page_url() -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
fn clear_location_hash() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_hash("");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_location_hash() {}

fn parse_hash_params(hash: &str) -> std::collections::BTreeMap<String, String> {
    hash.trim_start_matches(['#', '?'])
        .split('&')
        .filter(|segment| !segment.is_empty())
        .filter_map(|segment| {
            let (key, value) = segment.split_once('=')?;
            Some((decode_fragment_value(key), decode_fragment_value(value)))
        })
        .collect()
}

fn decode_fragment_value(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = decode_hex_digit(bytes[index + 1]);
                let low = decode_hex_digit(bytes[index + 2]);
                if let (Some(high), Some(low)) = (high, low) {
                    decoded.push((high << 4) | low);
                    index += 3;
                } else {
                    decoded.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

fn decode_hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[component]
fn SkillPage(lang: String, id: String) -> Element {
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

fn short_repo_name(url: &str) -> &str {
    let url = url.trim_end_matches('/').trim_end_matches(".git");
    url.rsplit('/').next().unwrap_or(url)
}

#[component]
fn IndexPage(lang: String) -> Element {
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum UserProfilePanel {
    Profile,
    PublishedSkills,
    StarredSkills,
    StarredFlocks,
    RecentHistory,
}

#[component]
fn UserPage(lang: String, handle: String) -> Element {
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

#[component]
fn DocsPage(lang: String, path: Vec<String>) -> Element {
    let joined = if path.is_empty() {
        "/".to_string()
    } else {
        path.join("/")
    };
    render_docs_page(lang, joined)
}

fn render_docs_page(lang: String, path: String) -> Element {
    let api = use_context::<ApiContext>();
    let t = use_context::<I18nContext>().t();
    let token = api.token.read().clone();

    // Build the API URL as a signal-like value that changes with props
    let api_url = if path == "/" || path.is_empty() {
        format!("/docs/{lang}")
    } else {
        format!("/docs/{lang}/{path}")
    };
    let mut url_sig = use_signal(move || api_url.clone());
    // Update signal when props change (re-render triggers this)
    let current_url = if path == "/" || path.is_empty() {
        format!("/docs/{lang}")
    } else {
        format!("/docs/{lang}/{path}")
    };
    if *url_sig.read() != current_url {
        url_sig.set(current_url);
    }

    let resource = use_resource(move || {
        let token = token.clone();
        let api_path = url_sig.read().clone();
        async move {
            api::get_json::<DocPageResponse>(api.api_base, token_option(&token), &api_path).await
        }
    });

    match &*resource.read_unchecked() {
        Some(Ok(page)) => {
            let base = format!("/{lang}/docs");

            rsx! {
                document::Title { "{page.title} - Savhub Docs" }
                section { class: "section",
                    div { class: "docs-layout",
                        nav { class: "docs-sidebar",
                            for group in page.sidebar.iter() {
                                div { class: "docs-sidebar-group",
                                    div { class: "docs-sidebar-title", "{group.title}" }
                                    for link in group.items.iter() {
                                        {
                                            let href = format!("{base}{}", link.link);
                                            let text = link.text.clone();
                                            rsx! {
                                                Link { to: "{href}", class: "docs-sidebar-link", "{text}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "docs-main",
                            div {
                                class: "docs-content",
                                dangerous_inner_html: "{page.content_html}",
                            }
                        }
                        if !page.toc.is_empty() {
                            aside { class: "docs-toc",
                                div { class: "docs-toc-title", "On this page" }
                                for item in page.toc.iter() {
                                    {
                                        let cls = match item.depth {
                                            3 => "docs-toc-link toc-h3",
                                            4 => "docs-toc-link toc-h4",
                                            _ => "docs-toc-link",
                                        };
                                        let href = format!("#{}", item.id);
                                        let text = item.text.clone();
                                        rsx! {
                                            a { href: "{href}", class: "{cls}", "{text}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(Err(error)) => rsx! {
            section { class: "section",
                p { class: "error", "{error}" }
            }
        },
        None => rsx! {
            section { class: "section",
                p { "{t.loading}" }
            }
        },
    }
}

#[component]
fn ManagementPage(lang: String, tab: String) -> Element {
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
fn AdminOverviewPage(lang: String) -> Element {
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
fn AdminUsersPage(lang: String) -> Element {
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
fn AdminIndexRulesPage(lang: String) -> Element {
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

#[component]
fn CommentComposer(
    body: String,
    placeholder: &'static str,
    on_input: EventHandler<String>,
    on_submit: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "composer",
            textarea {
                value: "{body}",
                placeholder: "{placeholder}",
                oninput: move |event| on_input.call(event.value())
            }
            button { class: "secondary", onclick: move |_| on_submit.call(()), {use_context::<I18nContext>().t().send} }
        }
    }
}

#[component]
fn MetadataPanel(metadata: BundleMetadata) -> Element {
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
fn CommentsList(
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

#[component]
fn StatTile(label: &'static str, value: i64) -> Element {
    rsx! {
        div { class: "stat-tile",
            strong { "{value}" }
            span { "{label}" }
        }
    }
}
