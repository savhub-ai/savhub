//! Embedded documentation serving as JSON API.
//!
//! Uses novel-core to parse markdown. Returns structured JSON
//! so the Dioxus frontend can render docs inside the SPA shell.

use novel_core::{BuiltSite, EmbedNovel, Novel};
use novel_shared::SidebarItem;
use once_cell::sync::Lazy;
use rust_embed::Embed;
use shared::{DocPageResponse, DocSidebarGroup, DocSidebarLink, DocTocItem};

#[derive(Embed)]
#[folder = "../../docs/en/"]
struct DocsEn;

#[derive(Embed)]
#[folder = "../../docs/zh/"]
struct DocsZh;

struct DocSites {
    en: BuiltSite,
    zh: BuiltSite,
}

static SITES: Lazy<DocSites> = Lazy::new(|| {
    let en = EmbedNovel::<DocsEn>::new()
        .title("Savhub Docs")
        .base("/docs/en/")
        .build()
        .unwrap_or_else(|err| {
            tracing::error!(
                "failed to build EN docs from embedded ../../docs/en/: {err}. \
                 Verify the docs directory is present at build time."
            );
            panic!("failed to build EN docs: {err}");
        });
    tracing::info!("[docs] EN site built with {} pages", en.pages().len());
    let zh = EmbedNovel::<DocsZh>::new()
        .title("Savhub 文档")
        .base("/docs/zh/")
        .build()
        .unwrap_or_else(|err| {
            tracing::error!(
                "failed to build ZH docs from embedded ../../docs/zh/: {err}. \
                 Verify the docs directory is present at build time."
            );
            panic!("failed to build ZH docs: {err}");
        });
    tracing::info!("[docs] ZH site built with {} pages", zh.pages().len());
    DocSites { en, zh }
});

fn site(lang: &str) -> &'static BuiltSite {
    match lang {
        "zh" => &SITES.zh,
        _ => &SITES.en,
    }
}

/// Look up a doc page and return structured JSON data.
pub fn get_page(lang: &str, route: &str) -> Option<DocPageResponse> {
    let s = site(lang);
    let normalized = if route.is_empty() || route == "/" {
        "/".to_string()
    } else {
        format!("/{}", route.trim_matches('/'))
    };
    let page = s.page(&normalized)?;

    let toc: Vec<DocTocItem> = page
        .toc
        .iter()
        .map(|t| DocTocItem {
            id: t.id.clone(),
            text: t.text.clone(),
            depth: t.depth,
        })
        .collect();

    let sidebar = build_sidebar_groups(s.sidebar());

    Some(DocPageResponse {
        title: page.title.clone(),
        description: page.description.clone(),
        content_html: page.content_html.clone(),
        toc,
        sidebar,
    })
}

/// List all available route paths for a language.
pub fn list_routes(lang: &str) -> Vec<String> {
    site(lang)
        .pages()
        .iter()
        .map(|p| p.route.route_path.clone())
        .collect()
}

fn build_sidebar_groups(
    sidebar_map: &std::collections::HashMap<String, Vec<SidebarItem>>,
) -> Vec<DocSidebarGroup> {
    let mut groups = Vec::new();
    for (title, items) in sidebar_map {
        let mut links = Vec::new();
        collect_links(items, &mut links);
        groups.push(DocSidebarGroup {
            title: title.clone(),
            items: links,
        });
    }
    // Sort by the canonical section order instead of alphabetically.
    fn section_order(title: &str) -> usize {
        let lower = title.to_lowercase();
        if lower.contains("getting") || lower.contains("快速") || lower.contains("入门") {
            0
        } else if lower.contains("client") || lower.contains("客户端") {
            1
        } else if lower.contains("developer") || lower.contains("开发") {
            2
        } else {
            3
        }
    }
    groups.sort_by_key(|g| section_order(&g.title));
    groups
}

fn collect_links(items: &[SidebarItem], out: &mut Vec<DocSidebarLink>) {
    for item in items {
        match item {
            SidebarItem::Link { text, link } => {
                out.push(DocSidebarLink {
                    text: text.clone(),
                    link: link.clone(),
                });
            }
            SidebarItem::Group { items, .. } => {
                collect_links(items, out);
            }
            SidebarItem::Divider => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn en_site_builds_and_serves_pages() {
        let routes = list_routes("en");
        assert!(!routes.is_empty(), "expected at least one EN route");
        let index = get_page("en", "/").expect("EN index page should exist");
        assert!(
            !index.title.is_empty(),
            "EN index title should not be empty"
        );
        assert!(
            !index.content_html.is_empty(),
            "EN index content_html should not be empty"
        );
    }

    #[test]
    fn zh_site_builds_and_serves_pages() {
        let routes = list_routes("zh");
        assert!(!routes.is_empty(), "expected at least one ZH route");
        let index = get_page("zh", "/").expect("ZH index page should exist");
        assert!(
            !index.title.is_empty(),
            "ZH index title should not be empty"
        );
        assert!(
            !index.content_html.is_empty(),
            "ZH index content_html should not be empty"
        );
    }
}
