//! Shared context / state types used across the Dioxus app tree.
//!
//! Extracted from `app.rs` to keep that file focused on routing and
//! page components.

use dioxus::prelude::*;
use savhub_shared::UserRole;

use crate::i18n::{self, Lang, T};

#[derive(Clone, Copy)]
pub(crate) struct ApiContext {
    pub api_base: &'static str,
    pub token: Signal<String>,
    pub auth_notice: Signal<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct I18nContext {
    pub lang: Signal<Lang>,
}

impl I18nContext {
    pub fn t(&self) -> &'static T {
        i18n::translations(*self.lang.read())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ToastKind {
    Error,
    Success,
    Info,
}

#[derive(Clone, Debug)]
pub(crate) struct ToastItem {
    pub id: u64,
    pub kind: ToastKind,
    pub message: String,
}

#[derive(Clone, Copy)]
pub(crate) struct ToastContext {
    pub items: Signal<Vec<ToastItem>>,
    pub counter: Signal<u64>,
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

    pub fn error(self, message: impl Into<String>) {
        self.push(ToastKind::Error, message);
    }

    pub fn success(self, message: impl Into<String>) {
        self.push(ToastKind::Success, message);
    }

    #[allow(dead_code)]
    pub fn info(self, message: impl Into<String>) {
        self.push(ToastKind::Info, message);
    }

    pub fn dismiss(mut self, id: u64) {
        self.items.write().retain(|t| t.id != id);
    }
}

/// Which admin tab is active, shared between topbar nav and ManagementPage.
#[derive(Clone, Copy)]
pub(crate) struct AdminTabCtx {
    // Read through Signal::read() at the call sites; the dead-code lint can't
    // see through Signal indirection.
    #[allow(dead_code)]
    pub tab: Signal<&'static str>,
}

/// Latest scan progress event from WebSocket, keyed by job_id.
#[derive(Clone, Copy)]
pub(crate) struct WsIndexEvents {
    pub events: Signal<std::collections::BTreeMap<String, IndexProgressEvent>>,
    /// Accumulated progress message log per job_id (distinct, ordered).
    pub message_log: Signal<std::collections::BTreeMap<String, Vec<String>>>,
}

/// Wire shape of the scan progress events streamed over WebSocket.
/// `result_data` is preserved for future inspection even if unused now.
#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct IndexProgressEvent {
    pub job_id: uuid::Uuid,
    pub status: String,
    pub progress_pct: i32,
    pub progress_message: String,
    #[allow(dead_code)]
    pub result_data: serde_json::Value,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InitialAuthState {
    pub token: String,
    pub notice: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredAuthSession {
    pub token: String,
    pub handle: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: UserRole,
    pub last_used_at: i64,
}
