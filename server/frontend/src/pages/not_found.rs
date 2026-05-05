use dioxus::prelude::*;

use crate::app::LANG_STORAGE_KEY;
use crate::contexts::I18nContext;
use crate::storage::load_storage;

#[component]
pub(crate) fn NotFound(segments: Vec<String>) -> Element {
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
            p {
                "{t.not_found_detail_prefix}"
                code { "{joined}" }
                "{t.not_found_detail_suffix}"
            }
        }
    }
}
