use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::DocPageResponse;

use crate::api;
use crate::app::token_option;
use crate::contexts::{ApiContext, I18nContext};

#[component]
pub(crate) fn DocsPage(lang: String, path: Vec<String>) -> Element {
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
