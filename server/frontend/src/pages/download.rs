use dioxus::prelude::*;

use crate::app::{
    CLIENT_LATEST_RELEASE_URL, CLIENT_RELEASES_URL, CLIENT_REPO_URL, INSTALL_PS1_URL,
    INSTALL_SH_URL,
};
use crate::contexts::I18nContext;
use crate::pages::widgets::InstallBlock;

#[component]
pub(crate) fn DownloadPage(lang: String) -> Element {
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
