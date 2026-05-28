//! WebSocket connection to `/api/v1/ws` for streaming index/scan progress.
//!
//! Maintains a single connection per browser tab with exponential-backoff
//! reconnect. Subscriptions are tracked so they can be replayed after a
//! disconnect.

#[cfg(target_arch = "wasm32")]
use dioxus::prelude::*;

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
pub(crate) fn start_ws_connection(ws_events: crate::contexts::WsIndexEvents) {
    connect_ws(ws_events);
}

#[cfg(target_arch = "wasm32")]
fn connect_ws(mut ws_events: crate::contexts::WsIndexEvents) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    let origin = crate::location::current_origin().unwrap_or_default();
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
                if let Ok(evt) = serde_json::from_str::<crate::contexts::IndexProgressEvent>(&text)
                {
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
fn schedule_ws_reconnect(ws_events: crate::contexts::WsIndexEvents) {
    use wasm_bindgen::JsCast;
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
pub(crate) fn start_ws_connection(_ws_events: crate::contexts::WsIndexEvents) {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn ws_subscribe(job_id: &str) {
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
pub(crate) fn ws_subscribe(_job_id: &str) {}
