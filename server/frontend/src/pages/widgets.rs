//! Small reusable widgets used by multiple pages.

use dioxus::prelude::*;

use crate::contexts::I18nContext;

#[component]
pub(crate) fn InstallBlock(label: &'static str, command: String) -> Element {
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
pub(crate) fn CommentComposer(
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
pub(crate) fn StatTile(label: &'static str, value: i64) -> Element {
    rsx! {
        div { class: "stat-tile",
            strong { "{value}" }
            span { "{label}" }
        }
    }
}
