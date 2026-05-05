//! Thin wrappers around `localStorage` with no-op fallbacks for non-wasm
//! builds (so cargo check on the host still compiles cleanly).

pub(crate) fn browser_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

pub(crate) fn load_storage(key: &str) -> Option<String> {
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

pub(crate) fn save_storage(key: &str, value: &str) {
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

pub(crate) fn remove_storage(key: &str) {
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
