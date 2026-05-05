use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dioxus::prelude::*;
use savhub_local::config::{add_project, read_projects_list, remove_project};
use savhub_local::project::{
    ProjectSelectorMatch, ResolvedProjectSkill, ResolvedSkillSources, disable_project_skill,
    enable_fetched_flock_in_project, enable_fetched_skill_in_project,
    read_project_selector_matches, resolve_project_skills_with_sources,
};

fn strip_url_scheme(url: &str) -> &str {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}

use crate::components::pagination::{self, PaginationControls};
use crate::i18n;
use crate::state::AppState;
use crate::theme::Theme;

const PROJECTS_PAGE_SIZE: usize = 10;
const PROJECT_SELECTORS_PAGE_SIZE: usize = 8;
const PROJECT_SKILLS_PAGE_SIZE: usize = 8;

#[component]
fn FolderBrowseIcon(size: u32) -> Element {
    rsx! { crate::icons::LucideIcon { icon: crate::icons::Icon::FolderPlus, size } }
}

#[component]
fn FolderOpenIcon(size: u32) -> Element {
    rsx! { crate::icons::LucideIcon { icon: crate::icons::Icon::FolderOpen, size } }
}

#[component]
pub fn ProjectsPage() -> Element {
    let state = use_context::<AppState>();
    let t = i18n::texts(*state.lang.read());
    let mut version = use_signal(|| 0u32);
    let mut selected_project = use_signal(|| Option::<String>::None);
    let mut projects_page = use_signal(|| 0usize);
    let mut browsing_project = use_signal(|| false);

    let _ = *version.read();

    let projects = read_projects_list().unwrap_or_default();
    let projects_items = projects.projects.clone();
    let current_projects_page = pagination::clamp_page(
        *projects_page.read(),
        projects_items.len(),
        PROJECTS_PAGE_SIZE,
    );
    let visible_projects =
        pagination::slice_for_page(&projects_items, current_projects_page, PROJECTS_PAGE_SIZE);
    let projects_total_pages = pagination::total_pages(projects_items.len(), PROJECTS_PAGE_SIZE);
    let title = t.projects_title;

    let do_browse_project = move |_| {
        if *browsing_project.read() {
            return;
        }

        browsing_project.set(true);
        let initial_dir = selected_project.read().clone();
        let dialog_title = t.projects_add.to_string();
        let mut browsing_project_signal = browsing_project;
        let mut selected_project_signal = selected_project;
        let mut version_signal = version;

        spawn(async move {
            let mut dialog = rfd::AsyncFileDialog::new().set_title(&dialog_title);

            if let Some(dir) = initial_dir {
                let dir = PathBuf::from(dir);
                if dir.is_dir() {
                    dialog = dialog.set_directory(&dir);
                }
            }

            if let Some(folder) = dialog.pick_folder().await {
                let path = folder.path().display().to_string();
                let _ = add_project(&path);
                selected_project_signal.set(Some(path));
                version_signal.with_mut(|v| *v += 1);
            }

            browsing_project_signal.set(false);
        });
    };

    let sel = selected_project.read().clone();
    let is_browsing_project = *browsing_project.read();
    let add_button_color = if is_browsing_project {
        Theme::MUTED
    } else {
        Theme::ACCENT_STRONG
    };
    let add_button_cursor = if is_browsing_project {
        "default"
    } else {
        "pointer"
    };
    let add_button_opacity = if is_browsing_project { "0.7" } else { "1" };
    let selected_project_detail = sel.as_ref().map(|project_path| {
        (
            project_path.clone(),
            format!("{}:{}", project_path, *version.read()),
        )
    });

    rsx! {
        div { style: "display: flex; height: 100%;",
            div { style: "width: 280px; background: rgba(238, 246, 232, 0.5); border-right: 1px solid {Theme::LINE}; display: flex; flex-direction: column; overflow: hidden;",
                div { style: "display: flex; align-items: center; justify-content: space-between; padding: 20px 16px 12px;",
                    h2 { style: "font-size: 18px; font-weight: 700; color: {Theme::TEXT};",
                        "{title}"
                    }
                    div { style: "display: flex; align-items: center; gap: 8px; margin-left: auto;",
                        PaginationControls {
                            current_page: current_projects_page,
                            total_pages: Some(projects_total_pages),
                            has_prev: current_projects_page > 0,
                            has_next: current_projects_page + 1 < projects_total_pages,
                            on_prev: move |_| projects_page.set(current_projects_page.saturating_sub(1)),
                            on_next: move |_| projects_page.set(current_projects_page + 1),
                        }
                        button {
                            style: "width: 30px; height: 30px; display: flex; align-items: center; justify-content: center; background: transparent; border: none; border-radius: 8px; cursor: {add_button_cursor}; padding: 0; line-height: 1; color: {add_button_color}; opacity: {add_button_opacity}; flex-shrink: 0;",
                            title: "{t.projects_add}",
                            disabled: is_browsing_project,
                            onclick: do_browse_project,
                            FolderBrowseIcon { size: 18 }
                        }
                    }
                }

                div { style: "flex: 1; overflow-y: auto; padding: 0 8px 8px;",
                    if projects_items.is_empty() {
                        p { style: "padding: 16px 8px; font-size: 12px; color: {Theme::MUTED}; text-align: center;",
                            "{t.projects_no_projects}"
                        }
                    } else {
                        for project in visible_projects.iter() {
                            {
                                let path_str = project.path.clone();
                                let path_for_select = project.path.clone();
                                let path_for_remove = project.path.clone();
                                let is_selected = sel.as_deref() == Some(project.path.as_str());
                                let dir_name = PathBuf::from(&project.path)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| project.path.clone());
                                let bg = if is_selected { Theme::ACCENT_LIGHT } else { "transparent" };
                                let text_color = if is_selected { Theme::ACCENT_STRONG } else { Theme::TEXT };

                                rsx! {
                                    div {
                                        style: "display: flex; align-items: center; justify-content: space-between; padding: 8px 12px; margin-bottom: 2px; background: {bg}; border-radius: 6px; cursor: pointer; transition: background 0.15s;",
                                        onclick: move |_| selected_project.set(Some(path_for_select.clone())),
                                        div { style: "flex: 1; min-width: 0;",
                                            p { style: "font-size: 13px; font-weight: 600; color: {text_color}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                                "{dir_name}"
                                            }
                                            p { style: "font-size: 10px; color: {Theme::MUTED}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                                "{path_str}"
                                            }
                                        }
                                        {
                                            let path_for_open = project.path.clone();
                                            rsx! {
                                                button {
                                                    style: "width: 28px; height: 28px; display: flex; align-items: center; justify-content: center; background: transparent; border: none; border-radius: 8px; color: {Theme::MUTED}; cursor: pointer; padding: 0; line-height: 1; flex-shrink: 0;",
                                                    title: "Open in explorer",
                                                    onclick: move |evt| {
                                                        evt.stop_propagation();
                                                        #[cfg(target_os = "windows")]
                                                        let _ = std::process::Command::new("explorer").arg(&path_for_open).spawn();
                                                        #[cfg(target_os = "macos")]
                                                        let _ = std::process::Command::new("open").arg(&path_for_open).spawn();
                                                        #[cfg(target_os = "linux")]
                                                        let _ = std::process::Command::new("xdg-open").arg(&path_for_open).spawn();
                                                    },
                                                    FolderOpenIcon { size: 16 }
                                                }
                                                button {
                                                    style: "background: none; border: none; color: {Theme::MUTED}; font-size: 14px; cursor: pointer; padding: 2px 4px; line-height: 1; flex-shrink: 0;",
                                                    onclick: move |evt| {
                                                        evt.stop_propagation();
                                                        let _ = remove_project(&path_for_remove);
                                                        if selected_project.read().as_deref() == Some(path_for_remove.as_str()) {
                                                            selected_project.set(None);
                                                        }
                                                        version.with_mut(|v| *v += 1);
                                                    },
                                                    crate::icons::LucideIcon { icon: crate::icons::Icon::X, size: 14 }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { style: "flex: 1; overflow-y: auto; padding: 24px;",
                if let Some((project_path, project_detail_key)) = selected_project_detail.clone() {
                    ProjectDetail {
                        key: "{project_detail_key}",
                        project_path: project_path.clone(),
                        version: version,
                    }
                } else {
                    div { style: "display: flex; align-items: center; justify-content: center; height: 200px;",
                        p { style: "font-size: 14px; color: {Theme::MUTED};",
                            "{t.project_select_hint}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ProjectDetail(project_path: String, mut version: Signal<u32>) -> Element {
    let state = use_context::<AppState>();
    let t = i18n::texts(*state.lang.read());
    let mut show_add_skill_dialog = use_signal(|| false);
    let mut show_add_flock_dialog = use_signal(|| false);
    let mut show_rescan_modal = use_signal(|| false);
    let mut selectors_page = use_signal(|| 0usize);
    let mut effective_skills_page = use_signal(|| 0usize);
    let mut detail_data = use_signal(|| Option::<ProjectDetailData>::None);
    let mut load_error = use_signal(|| Option::<String>::None);
    let mut loaded = use_signal(|| false);

    let _version = *version.read();

    let workdir = PathBuf::from(&project_path);

    let dir_name = workdir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project_path.clone());

    use_effect(move || {
        if *loaded.read() {
            return;
        }
        loaded.set(true);
        load_error.set(None);

        let workdir = workdir.clone();
        spawn(async move {
            match tokio::task::spawn_blocking(move || load_project_detail_data(&workdir)).await {
                Ok(data) => detail_data.set(Some(data)),
                Err(err) => load_error.set(Some(format!("failed to load project details: {err}"))),
            }
        });
    });

    let maybe_data = detail_data.read().as_ref().cloned();

    rsx! {
        div { style: "display: flex; align-items: flex-start; justify-content: space-between; margin-bottom: 20px;",
            div {
                h2 { style: "font-size: 20px; font-weight: 700; color: {Theme::TEXT}; margin-bottom: 4px;",
                    "{dir_name}"
                }
                p { style: "font-size: 12px; color: {Theme::MUTED};", "{project_path}" }
            }
            button {
                style: "padding: 7px 16px; background: linear-gradient(135deg, {Theme::ACCENT} 0%, #7bc25a 100%); color: white; border: none; border-radius: 8px; font-size: 13px; font-weight: 600; cursor: pointer; white-space: nowrap; flex-shrink: 0;",
                onclick: move |_| show_rescan_modal.set(true),
                "{t.project_rescan}"
            }
        }

        if *show_rescan_modal.read() {
            RescanModal { project_path: project_path.clone(), show: show_rescan_modal, version: version }
        }

        if let Some(err) = load_error.read().as_ref() {
            div { style: "background: rgba(139, 30, 30, 0.08); border: 1px solid rgba(139, 30, 30, 0.18); border-radius: 8px; padding: 16px; margin-bottom: 16px;",
                p { style: "font-size: 13px; color: {Theme::DANGER};", "{err}" }
            }
        } else if let Some(data) = maybe_data {
            {
                let selector_matches = data.selector_matches.clone();
                let effective_skills = data.effective_skills.clone();
                let repo_skills = data.repo_skills.clone();
                let enabled_skill_slugs = effective_skills
                    .iter()
                    .map(|skill| skill.slug.clone())
                    .collect::<BTreeSet<_>>();
                let selector_items = selector_matches.clone();
                let selector_current_page = pagination::clamp_page(
                    *selectors_page.read(),
                    selector_items.len(),
                    PROJECT_SELECTORS_PAGE_SIZE,
                );
                let visible_selector_matches = pagination::slice_for_page(
                    &selector_items,
                    selector_current_page,
                    PROJECT_SELECTORS_PAGE_SIZE,
                );
                let selector_total_pages =
                    pagination::total_pages(selector_items.len(), PROJECT_SELECTORS_PAGE_SIZE);
                let effective_skills_current_page = pagination::clamp_page(
                    *effective_skills_page.read(),
                    effective_skills.len(),
                    PROJECT_SKILLS_PAGE_SIZE,
                );
                let visible_effective_skills = pagination::slice_for_page(
                    &effective_skills,
                    effective_skills_current_page,
                    PROJECT_SKILLS_PAGE_SIZE,
                );
                let effective_skills_total_pages =
                    pagination::total_pages(effective_skills.len(), PROJECT_SKILLS_PAGE_SIZE);

                rsx! {
                    div { style: "background: {Theme::PANEL}; border: 1px solid {Theme::LINE}; border-radius: 8px; padding: 16px; margin-bottom: 16px;",
                        div { style: "display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 10px;",
                            h3 { style: "font-size: 13px; color: {Theme::MUTED};",
                                "{t.project_matched_selectors} ({selector_matches.len()})"
                            }
                            PaginationControls {
                                current_page: selector_current_page,
                                total_pages: Some(selector_total_pages),
                                has_prev: selector_current_page > 0,
                                has_next: selector_current_page + 1 < selector_total_pages,
                                on_prev: move |_| selectors_page.set(selector_current_page.saturating_sub(1)),
                                on_next: move |_| selectors_page.set(selector_current_page + 1),
                            }
                        }

                        if selector_matches.is_empty() {
                            p { style: "font-size: 13px; color: {Theme::MUTED};", "{t.project_no_selectors}" }
                        } else {
                            div { style: "display: flex; flex-direction: column; gap: 10px;",
                                for matched in visible_selector_matches.iter() {
                                    SelectorMatchRow {
                                        key: "{matched.selector}",
                                        selector: matched.selector.clone(),
                                    }
                                }
                            }
                        }
                    }

                    div { style: "background: {Theme::PANEL}; border: 1px solid {Theme::LINE}; border-radius: 8px; padding: 16px; margin-bottom: 16px;",
                        div { style: "display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 12px;",
                            h3 { style: "font-size: 13px; color: {Theme::MUTED};",
                                "{t.project_enabled_skills} ({effective_skills.len()})"
                            }
                            div { style: "display: flex; align-items: center; gap: 8px;",
                                PaginationControls {
                                    current_page: effective_skills_current_page,
                                    total_pages: Some(effective_skills_total_pages),
                                    has_prev: effective_skills_current_page > 0,
                                    has_next: effective_skills_current_page + 1 < effective_skills_total_pages,
                                    on_prev: move |_| effective_skills_page.set(effective_skills_current_page.saturating_sub(1)),
                                    on_next: move |_| effective_skills_page.set(effective_skills_current_page + 1),
                                }
                                button {
                                    style: "padding: 6px 12px; background: {Theme::BG_ELEVATED}; color: {Theme::TEXT}; border: 1px solid {Theme::LINE}; border-radius: 6px; font-size: 12px; font-weight: 600; cursor: pointer; white-space: nowrap;",
                                    onclick: move |_| show_add_flock_dialog.set(true),
                                    "{t.project_inject_flock}"
                                }
                                button {
                                    style: "padding: 6px 12px; background: linear-gradient(135deg, {Theme::ACCENT} 0%, #7bc25a 100%); color: white; border: none; border-radius: 6px; font-size: 12px; font-weight: 600; cursor: pointer; white-space: nowrap;",
                                    onclick: move |_| show_add_skill_dialog.set(true),
                                    "{t.project_inject_skill}"
                                }
                            }
                        }

                        if effective_skills.is_empty() {
                            p { style: "font-size: 13px; color: {Theme::MUTED};", "{t.project_no_enabled_skills}" }
                        } else {
                            div { style: "border: 1px solid {Theme::LINE}; border-radius: 8px; overflow: hidden;",
                                div { style: "display: grid; grid-template-columns: minmax(180px, 1.1fr) minmax(280px, 2fr) 96px; gap: 12px; padding: 10px 14px; background: rgba(238, 246, 232, 0.62); border-bottom: 1px solid {Theme::LINE};",
                                    p { style: "font-size: 12px; font-weight: 700; color: {Theme::TEXT};", "{t.project_skill_name}" }
                                    p { style: "font-size: 12px; font-weight: 700; color: {Theme::TEXT};", "{t.project_skill_reason}" }
                                    p { style: "font-size: 12px; font-weight: 700; color: {Theme::TEXT}; text-align: right;", "{t.project_skill_action}" }
                                }
                                for skill in visible_effective_skills.iter() {
                                    {
                                        let slug_for_remove = skill.slug.clone();
                                        let pp = project_path.clone();
                                        let reason = build_skill_reason_text(t, &skill.sources);
                                        let has_manual_source = skill.sources.manual;
                                        rsx! {
                                            div { style: "display: grid; grid-template-columns: minmax(180px, 1.1fr) minmax(280px, 2fr) 96px; gap: 12px; padding: 12px 14px; align-items: center; border-bottom: 1px solid {Theme::LINE};",
                                                div { style: "min-width: 0;",
                                                    p { style: "font-size: 14px; font-weight: 600; color: {Theme::TEXT}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                                        "{skill.display_name}"
                                                    }
                                                    p { style: "font-size: 11px; color: {Theme::MUTED}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                                        "{skill.slug}"
                                                    }
                                                }
                                                p { style: "font-size: 12px; color: {Theme::TEXT}; line-height: 1.6;",
                                                    "{reason}"
                                                }
                                                div { style: "display: flex; justify-content: flex-end;",
                                                    if has_manual_source {
                                                        button {
                                                            style: "padding: 5px 10px; background: rgba(139, 30, 30, 0.08); color: {Theme::DANGER}; border: 1px solid rgba(139, 30, 30, 0.18); border-radius: 6px; font-size: 11px; font-weight: 600; cursor: pointer; white-space: nowrap;",
                                                            onclick: move |_| {
                                                                let pp = pp.clone();
                                                                let slug = slug_for_remove.clone();
                                                                spawn(async move {
                                                                    let result = tokio::task::spawn_blocking(move || {
                                                                        let wd = PathBuf::from(&pp);
                                                                        disable_project_skill(&wd, &slug)
                                                                    }).await;
                                                                    match result {
                                                                        Ok(Ok(_)) => version.with_mut(|v| *v += 1),
                                                                        Ok(Err(e)) => eprintln!("failed to remove skill: {e}"),
                                                                        Err(e) => eprintln!("failed to remove skill: {e}"),
                                                                    }
                                                                });
                                                            },
                                                            "{t.projects_remove}"
                                                        }
                                                    } else {
                                                        span { style: "font-size: 12px; color: {Theme::MUTED};", "-" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if *show_add_skill_dialog.read() {
                        AddProjectSkillDialog {
                            project_path: project_path.clone(),
                            version: version,
                            skills: repo_skills,
                            enabled_skill_slugs: enabled_skill_slugs.into_iter().collect(),
                            add_label: t.project_inject_skill,
                            enabled_label: t.fetched,
                            empty_label: t.project_local_skills_empty,
                            title: t.project_local_skills_title,
                            on_close: move |_| show_add_skill_dialog.set(false),
                        }
                    }

                    if *show_add_flock_dialog.read() {
                        AddProjectFlockDialog {
                            project_path: project_path.clone(),
                            version: version,
                            flocks: data.flock_options.clone(),
                            already_added_tokens: data.added_flock_tokens.clone(),
                            on_close: move |_| show_add_flock_dialog.set(false),
                        }
                    }
                }
            }
        } else {
            div { style: "background: {Theme::PANEL}; border: 1px solid {Theme::LINE}; border-radius: 8px; padding: 24px; color: {Theme::MUTED};",
                "{t.loading}"
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ProjectDetailData {
    selector_matches: Vec<ProjectSelectorMatch>,
    effective_skills: Vec<ResolvedProjectSkill>,
    repo_skills: Vec<RepoSkillOption>,
    flock_options: Vec<RepoFlockOption>,
    added_flock_tokens: Vec<String>,
}

fn load_project_detail_data(workdir: &Path) -> ProjectDetailData {
    let cfg = savhub_local::project::read_project_config(workdir).unwrap_or_default();
    ProjectDetailData {
        selector_matches: read_project_selector_matches(workdir).unwrap_or_default(),
        effective_skills: resolve_project_skills_with_sources(workdir).unwrap_or_default(),
        repo_skills: collect_repo_skill_options(workdir),
        flock_options: collect_repo_flock_options(),
        added_flock_tokens: cfg.flocks.manual_added.clone(),
    }
}

#[derive(Clone, PartialEq, Eq)]
struct RepoSkillOption {
    repo_url: String,
    path: String,
    slug: String,
    display_name: String,
    flock_slug: Option<String>,
}

fn collect_repo_skill_options(_workdir: &Path) -> Vec<RepoSkillOption> {
    let savhub_dir = savhub_local::config::get_config_dir()
        .unwrap_or_else(|_| savhub_local::clients::home_dir().join(".savhub"));
    let lock = savhub_local::skills::read_lockfile(&savhub_dir).unwrap_or_default();
    let mut options: Vec<RepoSkillOption> = savhub_local::skills::flatten_lockfile(&lock)
        .into_iter()
        .map(|e| RepoSkillOption {
            repo_url: e.repo_url,
            path: e.path.clone(),
            slug: e.slug.clone(),
            display_name: e.slug,
            flock_slug: Some(e.flock_path),
        })
        .collect();
    options.sort_by(|a, b| {
        a.flock_slug
            .cmp(&b.flock_slug)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    options
}

#[derive(Clone, PartialEq, Eq)]
struct RepoFlockOption {
    repo_url: String,
    flock_slug: String,
    skill_count: usize,
    token: String,
}

fn collect_repo_flock_options() -> Vec<RepoFlockOption> {
    let savhub_dir = savhub_local::config::get_config_dir()
        .unwrap_or_else(|_| savhub_local::clients::home_dir().join(".savhub"));
    let lock = savhub_local::skills::read_lockfile(&savhub_dir).unwrap_or_default();
    let mut options: Vec<RepoFlockOption> = lock
        .repos
        .iter()
        .flat_map(|repo| {
            let repo_url = repo.git_url.clone();
            repo.flocks.iter().map(move |flock| RepoFlockOption {
                repo_url: repo_url.clone(),
                flock_slug: flock.path.clone(),
                skill_count: flock.skills.len(),
                token: format!("{}:{}", repo_url, flock.path),
            })
        })
        .collect();
    options.sort_by(|a, b| {
        a.repo_url
            .cmp(&b.repo_url)
            .then_with(|| a.flock_slug.cmp(&b.flock_slug))
    });
    options
}

fn build_skill_reason_text(t: &i18n::Texts, sources: &ResolvedSkillSources) -> String {
    let mut parts = Vec::new();
    if !sources.selectors.is_empty() {
        parts.push(format!(
            "{}: {}",
            t.project_reason_selectors,
            sources.selectors.join(", ")
        ));
    }
    if !sources.flocks.is_empty() {
        parts.push(format!(
            "{}: {}",
            t.project_reason_flocks,
            sources.flocks.join(", ")
        ));
    }
    if sources.manual {
        parts.push(t.project_reason_manual.to_string());
    }

    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join("; ")
    }
}

#[component]
fn SelectorMatchRow(selector: String) -> Element {
    rsx! {
        div { style: "padding: 12px 14px; background: {Theme::BG_ELEVATED}; border: 1px solid {Theme::LINE}; border-radius: 8px;",
            p { style: "font-size: 14px; font-weight: 600; color: {Theme::TEXT};",
                "{selector}"
            }
        }
    }
}

#[component]
fn AddProjectSkillDialog(
    project_path: String,
    mut version: Signal<u32>,
    skills: Vec<RepoSkillOption>,
    enabled_skill_slugs: Vec<String>,
    title: &'static str,
    add_label: &'static str,
    enabled_label: &'static str,
    empty_label: &'static str,
    on_close: EventHandler<()>,
) -> Element {
    let state = use_context::<AppState>();
    let t = i18n::texts(*state.lang.read());
    let enabled = enabled_skill_slugs.into_iter().collect::<BTreeSet<_>>();
    let mut status_msg = use_signal(|| Option::<String>::None);
    let mut backdrop_pressed = use_signal(|| false);
    let mut search_query = use_signal(String::new);
    let mut grouped_view = use_signal(|| true);

    let query = search_query.read().trim().to_lowercase();
    let filtered_skills: Vec<&RepoSkillOption> = skills
        .iter()
        .filter(|skill| {
            if query.is_empty() {
                return true;
            }
            skill.display_name.to_lowercase().contains(&query)
                || skill.slug.to_lowercase().contains(&query)
                || skill.repo_url.to_lowercase().contains(&query)
                || skill
                    .flock_slug
                    .as_deref()
                    .map(|f| f.to_lowercase().contains(&query))
                    .unwrap_or(false)
        })
        .collect();

    let is_grouped = *grouped_view.read();
    let toggle_bg = if is_grouped {
        Theme::ACCENT
    } else {
        Theme::BG_ELEVATED
    };
    let toggle_color = if is_grouped { "white" } else { Theme::TEXT };

    rsx! {
        div {
            style: "position: fixed; inset: 0; background: rgba(26, 46, 24, 0.38); display: flex; align-items: center; justify-content: center; padding: 24px; z-index: 1000;",
            onmousedown: move |_| backdrop_pressed.set(true),
            onmouseup: move |_| { if *backdrop_pressed.read() { on_close.call(()); } backdrop_pressed.set(false); },
            div {
                style: "width: 100%; max-width: 760px; max-height: 90vh; display: flex; flex-direction: column; background: {Theme::PANEL}; border: 1px solid {Theme::LINE}; border-radius: 12px; box-shadow: 0 24px 64px rgba(26, 46, 24, 0.18); padding: 20px;",
                onmousedown: move |evt| evt.stop_propagation(),
                onmouseup: move |evt| evt.stop_propagation(),

                // Header: title + search + toggle + close
                div { style: "display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 14px; flex-shrink: 0;",
                    h2 { style: "font-size: 18px; font-weight: 700; color: {Theme::TEXT}; white-space: nowrap;",
                        "{title}"
                    }
                    div { style: "flex: 1; min-width: 0;",
                        input {
                            r#type: "text",
                            placeholder: t.search_placeholder,
                            style: "width: 100%; padding: 7px 12px; background: {Theme::BG_ELEVATED}; border: 1px solid {Theme::LINE}; border-radius: 8px; font-size: 13px; color: {Theme::TEXT}; outline: none;",
                            value: "{search_query}",
                            oninput: move |evt| search_query.set(evt.value()),
                        }
                    }
                    div { style: "display: flex; align-items: center; gap: 6px; flex-shrink: 0;",
                        button {
                            style: "padding: 6px 12px; background: {toggle_bg}; color: {toggle_color}; border: 1px solid {Theme::LINE}; border-radius: 6px; font-size: 12px; font-weight: 600; cursor: pointer; white-space: nowrap;",
                            onclick: move |_| grouped_view.set(!is_grouped),
                            "{t.grouped_label}"
                        }
                        button {
                            style: "width: 32px; height: 32px; display: flex; align-items: center; justify-content: center; background: transparent; border: 1px solid {Theme::LINE}; border-radius: 8px; font-size: 16px; color: {Theme::MUTED}; cursor: pointer;",
                            onclick: move |_| on_close.call(()),
                            crate::icons::LucideIcon { icon: crate::icons::Icon::X, size: 14 }
                        }
                    }
                }

                if let Some(message) = status_msg.read().as_ref() {
                    p { style: "font-size: 12px; color: {Theme::DANGER}; margin-bottom: 12px; flex-shrink: 0;",
                        "{message}"
                    }
                }

                if filtered_skills.is_empty() {
                    p { style: "font-size: 13px; color: {Theme::MUTED};",
                        "{empty_label}"
                    }
                } else if is_grouped {
                    // Grouped view: show only flock-level rows (not individual skills)
                    div { style: "display: flex; flex-direction: column; gap: 10px; overflow-y: auto; flex: 1; min-height: 0;",
                        {
                            let mut groups: Vec<(String, Vec<&RepoSkillOption>)> = Vec::new();
                            for skill in &filtered_skills {
                                let group_key = skill.flock_slug.clone()
                                    .unwrap_or_else(|| strip_url_scheme(&skill.repo_url).to_string());
                                if let Some(existing) = groups.iter_mut().find(|(k, _)| k == &group_key) {
                                    existing.1.push(skill);
                                } else {
                                    groups.push((group_key, vec![skill]));
                                }
                            }
                            rsx! {
                                for (group_name, group_skills) in groups.iter() {
                                    {
                                        let all_enabled = group_skills.iter().all(|s| enabled.contains(&s.slug));
                                        let enabled_count = group_skills.iter().filter(|s| enabled.contains(&s.slug)).count();
                                        let total_count = group_skills.len();
                                        rsx! {
                                            div { style: "display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 12px 14px; background: {Theme::BG_ELEVATED}; border: 1px solid {Theme::LINE}; border-radius: 8px;",
                                                div { style: "min-width: 0; flex: 1;",
                                                    p { style: "font-size: 14px; font-weight: 600; color: {Theme::TEXT}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                                        "{group_name}"
                                                    }
                                                    p { style: "font-size: 11px; color: {Theme::MUTED}; margin-top: 2px;",
                                                        "{enabled_count}/{total_count} skills"
                                                    }
                                                }
                                                div { style: "display: flex; align-items: center; gap: 8px; flex-shrink: 0;",
                                                    if all_enabled {
                                                        span { style: "font-size: 11px; font-weight: 600; color: {Theme::ACCENT_STRONG};",
                                                            "{enabled_label}"
                                                        }
                                                    }
                                                    button {
                                                        style: "padding: 6px 12px; background: linear-gradient(135deg, #6aa84f 0%, #7bc25a 100%); color: white; border: none; border-radius: 6px; font-size: 12px; font-weight: 600; cursor: pointer; white-space: nowrap;",
                                                        onclick: {
                                                            let project_path = project_path.clone();
                                                            let skills_to_add: Vec<RepoSkillOption> = group_skills.iter().filter(|s| !enabled.contains(&s.slug)).map(|s| (*s).clone()).collect();
                                                            move |_| {
                                                                let pp = project_path.clone();
                                                                let skills = skills_to_add.clone();
                                                                spawn(async move {
                                                                    let mut had_error = false;
                                                                    for sk in skills {
                                                                        let pp2 = pp.clone();
                                                                        let result = tokio::task::spawn_blocking(move || {
                                                                            let wd = PathBuf::from(&pp2);
                                                                            enable_fetched_skill_in_project(&wd, &sk.repo_url, &sk.path, &sk.slug)
                                                                        }).await;
                                                                        match result {
                                                                            Ok(Ok(_)) => {}
                                                                            Ok(Err(error)) => {
                                                                                status_msg.set(Some(error.to_string()));
                                                                                had_error = true;
                                                                                break;
                                                                            }
                                                                            Err(error) => {
                                                                                status_msg.set(Some(error.to_string()));
                                                                                had_error = true;
                                                                                break;
                                                                            }
                                                                        }
                                                                    }
                                                                    if !had_error {
                                                                        status_msg.set(None);
                                                                        version.with_mut(|value| *value += 1);
                                                                        on_close.call(());
                                                                    }
                                                                });
                                                            }
                                                        },
                                                        "{add_label}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Flat list
                    div { style: "display: flex; flex-direction: column; gap: 10px; overflow-y: auto; flex: 1; min-height: 0;",
                        for skill in filtered_skills.iter() {
                            {
                                let is_enabled = enabled.contains(&skill.slug);
                                let repo_display = strip_url_scheme(&skill.repo_url);
                                let group_label = skill.flock_slug.as_deref().unwrap_or(repo_display);
                                rsx! {
                                    div { style: "display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 12px 14px; background: {Theme::BG_ELEVATED}; border: 1px solid {Theme::LINE}; border-radius: 8px;",
                                        div { style: "min-width: 0; flex: 1;",
                                            p { style: "font-size: 14px; font-weight: 600; color: {Theme::TEXT}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                                "{skill.display_name}"
                                            }
                                            p { style: "font-size: 11px; color: {Theme::MUTED}; margin-top: 2px;",
                                                "{group_label} / {skill.slug}"
                                            }
                                            p { style: "font-size: 11px; color: {Theme::MUTED}; margin-top: 4px; font-family: Consolas, 'SFMono-Regular', monospace; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                                "{repo_display}/{skill.path}"
                                            }
                                        }
                                        div { style: "display: flex; align-items: center; gap: 8px; flex-shrink: 0;",
                                            if is_enabled {
                                                span { style: "font-size: 11px; font-weight: 600; color: {Theme::ACCENT_STRONG};",
                                                    "{enabled_label}"
                                                }
                                            }
                                            button {
                                                style: "padding: 6px 12px; background: linear-gradient(135deg, #6aa84f 0%, #7bc25a 100%); color: white; border: none; border-radius: 6px; font-size: 12px; font-weight: 600; cursor: pointer; white-space: nowrap;",
                                                onclick: {
                                                    let project_path = project_path.clone();
                                                    let skill = (*skill).clone();
                                                    move |_| {
                                                        let pp = project_path.clone();
                                                        let sk = skill.clone();
                                                        spawn(async move {
                                                            let result = tokio::task::spawn_blocking(move || {
                                                                let wd = PathBuf::from(&pp);
                                                                enable_fetched_skill_in_project(&wd, &sk.repo_url, &sk.path, &sk.slug)
                                                            }).await;
                                                            match result {
                                                                Ok(Ok(_)) => {
                                                                    status_msg.set(None);
                                                                    version.with_mut(|value| *value += 1);
                                                                    on_close.call(());
                                                                }
                                                                Ok(Err(error)) => status_msg.set(Some(error.to_string())),
                                                                Err(error) => status_msg.set(Some(error.to_string())),
                                                            }
                                                        });
                                                    }
                                                },
                                                "{add_label}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AddProjectFlockDialog(
    project_path: String,
    mut version: Signal<u32>,
    flocks: Vec<RepoFlockOption>,
    already_added_tokens: Vec<String>,
    on_close: EventHandler<()>,
) -> Element {
    let state = use_context::<AppState>();
    let t = i18n::texts(*state.lang.read());
    let added: BTreeSet<String> = already_added_tokens.into_iter().collect();
    let mut status_msg = use_signal(|| Option::<String>::None);
    let mut backdrop_pressed = use_signal(|| false);
    let mut search_query = use_signal(String::new);

    let query = search_query.read().trim().to_lowercase();
    let filtered: Vec<&RepoFlockOption> = flocks
        .iter()
        .filter(|flock| {
            if query.is_empty() {
                return true;
            }
            flock.flock_slug.to_lowercase().contains(&query)
                || flock.repo_url.to_lowercase().contains(&query)
        })
        .collect();

    rsx! {
        div {
            style: "position: fixed; inset: 0; background: rgba(26, 46, 24, 0.38); display: flex; align-items: center; justify-content: center; padding: 24px; z-index: 1000;",
            onmousedown: move |_| backdrop_pressed.set(true),
            onmouseup: move |_| { if *backdrop_pressed.read() { on_close.call(()); } backdrop_pressed.set(false); },
            div {
                style: "width: 100%; max-width: 720px; max-height: 80vh; display: flex; flex-direction: column; background: {Theme::PANEL}; border: 1px solid {Theme::LINE}; border-radius: 12px; box-shadow: 0 24px 64px rgba(26, 46, 24, 0.18); padding: 20px;",
                onmousedown: move |evt| evt.stop_propagation(),
                onmouseup: move |evt| evt.stop_propagation(),

                div { style: "display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 14px; flex-shrink: 0;",
                    h2 { style: "font-size: 18px; font-weight: 700; color: {Theme::TEXT}; white-space: nowrap;",
                        "{t.project_local_flocks_title}"
                    }
                    div { style: "flex: 1; min-width: 0;",
                        input {
                            r#type: "text",
                            placeholder: t.search_placeholder,
                            style: "width: 100%; padding: 7px 12px; background: {Theme::BG_ELEVATED}; border: 1px solid {Theme::LINE}; border-radius: 8px; font-size: 13px; color: {Theme::TEXT}; outline: none;",
                            value: "{search_query}",
                            oninput: move |evt| search_query.set(evt.value()),
                        }
                    }
                    button {
                        style: "width: 32px; height: 32px; display: flex; align-items: center; justify-content: center; background: transparent; border: 1px solid {Theme::LINE}; border-radius: 8px; font-size: 16px; color: {Theme::MUTED}; cursor: pointer;",
                        onclick: move |_| on_close.call(()),
                        crate::icons::LucideIcon { icon: crate::icons::Icon::X, size: 14 }
                    }
                }

                if let Some(message) = status_msg.read().as_ref() {
                    p { style: "font-size: 12px; color: {Theme::DANGER}; margin-bottom: 12px; flex-shrink: 0;",
                        "{message}"
                    }
                }

                if filtered.is_empty() {
                    p { style: "font-size: 13px; color: {Theme::MUTED};",
                        "{t.project_local_flocks_empty}"
                    }
                } else {
                    div { style: "display: flex; flex-direction: column; gap: 10px; overflow-y: auto; flex: 1; min-height: 0;",
                        for flock in filtered.iter() {
                            {
                                let token = flock.token.clone();
                                let repo_display = strip_url_scheme(&flock.repo_url).to_string();
                                let slug = flock.flock_slug.clone();
                                let count = flock.skill_count;
                                let already = added.contains(&token);
                                let pp = project_path.clone();
                                let repo_url = flock.repo_url.clone();
                                let slug_for_apply = slug.clone();
                                rsx! {
                                    div { style: "display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 12px 14px; background: {Theme::BG_ELEVATED}; border: 1px solid {Theme::LINE}; border-radius: 8px;",
                                        div { style: "min-width: 0; flex: 1;",
                                            p { style: "font-size: 14px; font-weight: 600; color: {Theme::TEXT}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                                "{slug}"
                                            }
                                            p { style: "font-size: 11px; color: {Theme::MUTED}; margin-top: 2px; font-family: Consolas, 'SFMono-Regular', monospace; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                                "{repo_display}"
                                            }
                                            p { style: "font-size: 11px; color: {Theme::MUTED}; margin-top: 2px;",
                                                "{count} skills"
                                            }
                                        }
                                        div { style: "display: flex; align-items: center; gap: 8px; flex-shrink: 0;",
                                            if already {
                                                span { style: "font-size: 11px; font-weight: 600; color: {Theme::ACCENT_STRONG};",
                                                    "{t.fetched}"
                                                }
                                            }
                                            button {
                                                style: "padding: 6px 12px; background: linear-gradient(135deg, #6aa84f 0%, #7bc25a 100%); color: white; border: none; border-radius: 6px; font-size: 12px; font-weight: 600; cursor: pointer; white-space: nowrap;",
                                                onclick: move |_| {
                                                    let pp = pp.clone();
                                                    let repo_url = repo_url.clone();
                                                    let slug = slug_for_apply.clone();
                                                    spawn(async move {
                                                        let result = tokio::task::spawn_blocking(move || {
                                                            let wd = PathBuf::from(&pp);
                                                            enable_fetched_flock_in_project(&wd, &repo_url, &slug)
                                                        }).await;
                                                        match result {
                                                            Ok(Ok(warnings)) => {
                                                                if warnings.is_empty() {
                                                                    status_msg.set(None);
                                                                } else {
                                                                    status_msg.set(Some(warnings.join("; ")));
                                                                }
                                                                version.with_mut(|value| *value += 1);
                                                                on_close.call(());
                                                            }
                                                            Ok(Err(error)) => status_msg.set(Some(error.to_string())),
                                                            Err(error) => status_msg.set(Some(error.to_string())),
                                                        }
                                                    });
                                                },
                                                "{t.project_inject_flock}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rescan & Apply modal
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct ScanData {
    matched_names: Vec<String>,
    flock_refs: Vec<String>,
    skill_refs: Vec<String>,
}

#[component]
fn RescanModal(project_path: String, mut show: Signal<bool>, mut version: Signal<u32>) -> Element {
    let state = use_context::<AppState>();
    let t = i18n::texts(*state.lang.read());

    let mut scan_data = use_signal(ScanData::default);
    let mut scan_loaded = use_signal(|| false);

    // Run selectors off the UI thread
    use_effect({
        let project_path = project_path.clone();
        move || {
            if *scan_loaded.read() {
                return;
            }
            scan_loaded.set(true);
            let pp = project_path.clone();
            spawn(async move {
                let data = tokio::task::spawn_blocking(move || {
                    let workdir = PathBuf::from(&pp);
                    let scan_result = savhub_local::selectors::run_selectors(&workdir).ok();
                    if let Some(result) = scan_result {
                        let matched: Vec<String> = result
                            .matched
                            .iter()
                            .map(|m| m.selector.name.clone())
                            .collect();
                        let flocks: Vec<String> =
                            result.flocks.iter().map(|s| s.to_string()).collect();
                        let mut skills: Vec<String> =
                            result.skills.iter().map(|s| s.to_string()).collect();
                        for flock_ref in &result.flocks {
                            if let Ok(flock_skills) = savhub_local::registry::list_flock_skills(
                                &flock_ref.repo,
                                &flock_ref.path,
                            ) {
                                for s in flock_skills {
                                    if !skills.contains(&s) {
                                        skills.push(s);
                                    }
                                }
                            }
                        }
                        ScanData {
                            matched_names: matched,
                            flock_refs: flocks,
                            skill_refs: skills,
                        }
                    } else {
                        ScanData::default()
                    }
                })
                .await
                .unwrap_or_default();
                scan_data.set(data);
            });
        }
    });

    let data = scan_data.read();
    let matched_names = data.matched_names.clone();
    let flock_refs = data.flock_refs.clone();
    let skill_refs = data.skill_refs.clone();
    let has_match = !matched_names.is_empty();
    drop(data);

    // Resolve AI agents with checkboxes
    let configured_agents = state.agents.read().clone();
    let all_clients = savhub_local::clients::resolve_clients(&configured_agents);
    let client_names: Vec<(String, String, bool)> = all_clients
        .iter()
        .filter(|c| c.kind.project_skills_dir().is_some())
        .map(|c| (c.kind.as_str().to_string(), c.name.clone(), c.installed))
        .collect();

    let mut agent_checks = use_signal(|| {
        client_names
            .iter()
            .map(|(id, _, installed)| (id.clone(), *installed))
            .collect::<Vec<(String, bool)>>()
    });
    let mut apply_status = use_signal(|| 0u8); // 0=idle, 1=applying, 2=done

    let project_path_clone = project_path.clone();
    let skill_refs_cl = skill_refs.clone();
    let matched_names_cl = matched_names.clone();
    let flock_refs_cl = flock_refs.clone();
    let do_apply = move |_| {
        apply_status.set(1);
        let workdir = PathBuf::from(&project_path_clone);

        let checked: Vec<String> = agent_checks
            .read()
            .iter()
            .filter(|(_, on)| *on)
            .map(|(id, _)| id.clone())
            .collect();

        let skills_to_install = skill_refs_cl.clone();
        let matched_data = matched_names_cl.clone();
        let _flock_data = flock_refs_cl.clone();
        spawn(async move {
            let workdir_bg = workdir.clone();
            let checked_bg = checked.clone();
            let _ = tokio::task::spawn_blocking(
                move || -> Result<(), Box<dyn std::error::Error + Send>> {
                    if !matched_data.is_empty() {
                        if let Ok(mut cfg) = savhub_local::project::read_project_config(&workdir_bg)
                        {
                            // Re-run selectors in this thread to get full match data
                            if let Ok(result) = savhub_local::selectors::run_selectors(&workdir_bg)
                            {
                                cfg.selectors.matched = result
                                    .matched
                                    .iter()
                                    .map(|m| savhub_local::project::ProjectSelectorMatch {
                                        selector: m.selector.name.clone(),
                                        flocks: m.flocks.clone(),
                                        skills: m.skills.clone(),
                                        repos: m.repos.clone(),
                                    })
                                    .collect();
                            }
                            let _ = savhub_local::project::write_project_config(&workdir_bg, &cfg);
                        }

                        let config = savhub_local::project::read_project_config(&workdir_bg)
                            .unwrap_or_default();
                        let skipped = &config.skills.manual_skipped;
                        let filtered: Vec<String> = skills_to_install
                            .iter()
                            .filter(|s| !savhub_local::registry::skill_matches_skipped(s, skipped))
                            .cloned()
                            .collect();

                        let filtered_pairs: Vec<(String, String)> = filtered
                            .iter()
                            .map(|s| {
                                let r = savhub_local::selectors::SelectorSkillRef::parse(s);
                                (r.repo, r.path)
                            })
                            .collect();
                        if let Ok(results) =
                            savhub_local::registry::fetch_skills_batch(&filtered_pairs)
                        {
                            let agents = savhub_local::clients::resolve_clients(&checked_bg);
                            for info in &results {
                                for client in &agents {
                                    if !client.installed {
                                        continue;
                                    }
                                    let Some(rel_dir) = client.kind.project_skills_dir() else {
                                        continue;
                                    };
                                    let target = workdir_bg.join(rel_dir).join(&info.slug);
                                    let _ = std::fs::create_dir_all(target.parent().unwrap());
                                    let _ = savhub_local::skills::copy_skill_folder(
                                        &info.local_path,
                                        &target,
                                    );
                                }
                            }
                            let mut lock =
                                savhub_local::project::read_project_lockfile(&workdir_bg)
                                    .unwrap_or_default();
                            for info in &results {
                                if !lock.skills.iter().any(|s| s.slug == info.slug) {
                                    let vi = savhub_local::skills::read_skill_version_info(
                                        &info.local_path,
                                    )
                                    .unwrap_or_default();
                                    lock.skills.push(savhub_local::project::ProjectLockedSkill {
                                        repo: Some(info.repo_sign.clone()),
                                        path: Some(info.skill_path.clone()),
                                        slug: info.slug.clone(),
                                        version: vi.version,
                                        git_sha: vi.git_sha,
                                    });
                                }
                            }
                            let _ = savhub_local::project::write_project_lockfile(&workdir, &lock);
                        }
                    } else {
                        let lock = savhub_local::project::read_project_lockfile(&workdir_bg)
                            .unwrap_or_default();
                        let agents = savhub_local::clients::resolve_clients(&checked_bg);
                        for skill in &lock.skills {
                            for client in &agents {
                                if !client.installed {
                                    continue;
                                }
                                let Some(rel_dir) = client.kind.project_skills_dir() else {
                                    continue;
                                };
                                let _ = std::fs::remove_dir_all(
                                    workdir_bg.join(rel_dir).join(&skill.slug),
                                );
                            }
                        }
                        if let Ok(mut cfg) = savhub_local::project::read_project_config(&workdir_bg)
                        {
                            cfg.selectors.matched.clear();
                            let _ = savhub_local::project::write_project_config(&workdir_bg, &cfg);
                        }
                        let _ = std::fs::remove_file(workdir_bg.join("savhub.lock"));
                    }

                    let _ = savhub_local::config::add_project(&workdir_bg.display().to_string());
                    Ok(())
                },
            )
            .await;
            apply_status.set(2);
            version.with_mut(|v| *v += 1);
        }); // end spawn
    };

    let status = *apply_status.read();

    rsx! {
        div { style: "position: fixed; inset: 0; background: rgba(0,0,0,0.35); display: flex; align-items: center; justify-content: center; z-index: 100;",
            onclick: move |_| { if status != 1 { show.set(false); } },
            div {
                style: "background: {Theme::BG_ELEVATED}; border-radius: 16px; padding: 28px 32px; min-width: 480px; max-width: 600px; max-height: 80vh; overflow-y: auto; box-shadow: 0 8px 32px rgba(0,0,0,0.18);",
                onclick: move |e| e.stop_propagation(),

                h2 { style: "font-size: 18px; font-weight: 700; color: {Theme::TEXT}; margin-bottom: 16px;",
                    "{t.project_rescan_title}"
                }

                if !has_match {
                    p { style: "font-size: 14px; color: {Theme::MUTED}; margin-bottom: 16px;",
                        "{t.project_rescan_no_match}"
                    }
                } else {
                    div { style: "margin-bottom: 14px;",
                        h3 { style: "font-size: 13px; color: {Theme::MUTED}; margin-bottom: 6px;",
                            "{t.project_rescan_matched} ({matched_names.len()})"
                        }
                        for name in matched_names.iter() {
                            div { style: "padding: 4px 0; font-size: 13px; color: {Theme::ACCENT_STRONG};",
                                span { style: "display: inline-flex; align-items: center; gap: 3px;",
                                    crate::icons::LucideIcon { icon: crate::icons::Icon::Check, size: 12 }
                                    "{name}"
                                }
                            }
                        }
                    }

                    if !flock_refs.is_empty() {
                        div { style: "margin-bottom: 14px;",
                            h3 { style: "font-size: 13px; color: {Theme::MUTED}; margin-bottom: 6px;",
                                "{t.project_rescan_flocks}"
                            }
                            div { style: "display: flex; flex-wrap: wrap; gap: 6px;",
                                for f in flock_refs.iter() {
                                    span { style: "padding: 3px 10px; background: {Theme::ACCENT_LIGHT}; border-radius: 12px; font-size: 12px; color: {Theme::ACCENT_STRONG};",
                                        "{f}"
                                    }
                                }
                            }
                        }
                    }

                    if !skill_refs.is_empty() {
                        div { style: "margin-bottom: 14px;",
                            h3 { style: "font-size: 13px; color: {Theme::MUTED}; margin-bottom: 6px;",
                                "{t.project_rescan_skills} ({skill_refs.len()})"
                            }
                            div { style: "max-height: 120px; overflow-y: auto;",
                                for s in skill_refs.iter() {
                                    div { style: "padding: 2px 0; font-size: 13px; color: {Theme::TEXT};",
                                        "{s}"
                                    }
                                }
                            }
                        }
                    }
                }

                // AI Agents checkboxes
                div { style: "margin-bottom: 16px; padding-top: 10px; border-top: 1px solid {Theme::LINE};",
                    h3 { style: "font-size: 13px; color: {Theme::MUTED}; margin-bottom: 8px;",
                        "{t.settings_agents}"
                    }
                    for (idx , (_id , display_name , _installed)) in client_names.iter().enumerate() {
                        {
                            let checked = agent_checks.read().get(idx).map(|(_, v)| *v).unwrap_or(false);
                            let idx_copy = idx;
                            rsx! {
                                label { style: "display: flex; align-items: center; gap: 8px; padding: 4px 0; font-size: 13px; color: {Theme::TEXT}; cursor: pointer;",
                                    input {
                                        r#type: "checkbox",
                                        checked: checked,
                                        onchange: move |e: Event<FormData>| {
                                            let val = e.value() == "true";
                                            agent_checks.with_mut(|list| {
                                                if let Some(entry) = list.get_mut(idx_copy) {
                                                    entry.1 = val;
                                                }
                                            });
                                        },
                                    }
                                    "{display_name}"
                                }
                            }
                        }
                    }
                }

                // Action buttons
                div { style: "display: flex; gap: 10px; justify-content: flex-end;",
                    button {
                        style: "padding: 8px 20px; background: transparent; color: {Theme::MUTED}; border: 1px solid {Theme::LINE}; border-radius: 10px; font-size: 13px; font-weight: 600; cursor: pointer;",
                        disabled: status == 1,
                        onclick: move |_| show.set(false),
                        "{t.project_rescan_close}"
                    }
                    if status == 0 {
                        button {
                            style: "padding: 8px 20px; background: linear-gradient(135deg, {Theme::ACCENT} 0%, #7bc25a 100%); color: white; border: none; border-radius: 10px; font-size: 13px; font-weight: 700; cursor: pointer;",
                            onclick: do_apply,
                            "{t.project_rescan_apply}"
                        }
                    }
                    if status == 1 {
                        span { style: "padding: 8px 12px; font-size: 13px; color: {Theme::MUTED};",
                            "{t.project_rescan_applying}"
                        }
                    }
                    if status == 2 {
                        span { style: "padding: 8px 12px; font-size: 13px; color: {Theme::ACCENT_STRONG}; font-weight: 600;",
                            "{t.project_rescan_done}"
                        }
                    }
                }
            }
        }
    }
}
