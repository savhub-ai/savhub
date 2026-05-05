//! Browser `Location`, `History`, and viewport helpers.
//!
//! Each function has a wasm32 implementation that goes through `web_sys` and
//! a non-wasm fallback (mostly empty/None) so the binary still type-checks
//! when targeting the host. Keeps `app.rs` free of two implementations of
//! every location operation.

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_origin() -> Option<String> {
    web_sys::window()?.location().origin().ok()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_origin() -> Option<String> {
    None
}

/// Check whether the window is scrolled near the bottom (within `threshold` pixels).
#[cfg(target_arch = "wasm32")]
pub(crate) fn near_window_bottom(threshold: f64) -> bool {
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
pub(crate) fn near_window_bottom(_threshold: f64) -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_query() -> String {
    web_sys::window()
        .and_then(|window| window.location().search().ok())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_query() -> String {
    String::new()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn clear_location_query() {
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
pub(crate) fn clear_location_query() {}

/// Read a single query-string parameter from the current browser URL.
pub(crate) fn query_param(key: &str) -> Option<String> {
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
pub(crate) fn set_location_query(params: &[(&str, &str)]) {
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
pub(crate) fn set_location_query(_params: &[(&str, &str)]) {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_hash() -> String {
    web_sys::window()
        .and_then(|window| window.location().hash().ok())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_hash() -> String {
    String::new()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_page_url() -> Option<String> {
    let href = web_sys::window()?.location().href().ok()?;
    let mut url = reqwest::Url::parse(&href).ok()?;
    url.set_fragment(None);
    Some(url.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_page_url() -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn clear_location_hash() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_hash("");
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn clear_location_hash() {}

pub(crate) fn parse_hash_params(hash: &str) -> std::collections::BTreeMap<String, String> {
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
