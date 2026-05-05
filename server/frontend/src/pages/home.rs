use dioxus::prelude::*;
use dioxus_router::Link;
use savhub_shared::{FlockSummary, PagedResponse};

use crate::api;
use crate::app::{Route, render_flock_cards, token_option, url_lang};
use crate::contexts::{ApiContext, I18nContext};

#[component]
pub(crate) fn Home(lang: String) -> Element {
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
