//! `savhub apply` — scan project, match selectors, fetch matched skills,
//! sync them into each detected AI client's project skills directory.
//!
//! Extracted from `main.rs` so the 600+ line apply pipeline doesn't crowd
//! the command dispatcher and arg parsing.

use std::collections::BTreeSet;

use anyhow::Result;
use dialoguer::Confirm;
use savhub_local::registry;
use savhub_local::selectors::run_selectors;
use serde_json::json;

use crate::tui;
use crate::{ApplyArgs, GlobalOpts, optional_client};

pub(crate) fn cmd_apply(opts: &GlobalOpts, mut args: ApplyArgs) -> Result<()> {
    // Trim and deduplicate all list args
    fn clean(v: &mut Vec<String>) {
        *v = v
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        v.dedup();
    }
    clean(&mut args.agents);
    clean(&mut args.skip_agents);
    clean(&mut args.add_skills);
    clean(&mut args.skip_skills);
    clean(&mut args.add_flocks);
    clean(&mut args.skip_flocks);

    let workdir = &opts.workdir;

    // Sync official selectors from the server before scanning
    eprintln!("Syncing official selectors...");
    if let Err(e) = savhub_local::selectors::sync_official_selectors(&opts.api_base) {
        eprintln!("Warning: could not sync official selectors: {e}");
    }

    eprintln!("Scanning project...");
    let result = run_selectors(workdir)?;

    let existing_config = savhub_local::project::read_project_config(workdir)?;
    let has_manual_entries = !existing_config.skills.manual_added.is_empty()
        || !existing_config.flocks.manual_added.is_empty();

    if result.matched.is_empty() && !has_manual_entries {
        println!(
            "No selectors matched this project. All skills previously applied by savhub will be removed."
        );

        // Read savhub.lock for fetched skills
        let lockfile = savhub_local::project::read_project_lockfile(workdir)?;

        if !lockfile.skills.is_empty() {
            println!("\nSkills to remove:");
            for s in &lockfile.skills {
                println!("  \x1b[31m[-]\x1b[0m {}", s.slug);
            }

            if !args.yes && opts.input_allowed {
                let proceed = Confirm::new()
                    .with_prompt(format!(
                        "Remove {} skill(s) from AI client directories?",
                        lockfile.skills.len()
                    ))
                    .default(true)
                    .interact()?;
                if !proceed {
                    println!("Cancelled.");
                    return Ok(());
                }
            }

            // Remove skill folders from AI client project-level dirs
            let all_clients = savhub_local::clients::detect_clients();
            for skill in &lockfile.skills {
                let slug = skill.slug.as_str();
                for client in &all_clients {
                    if !client.installed {
                        continue;
                    }
                    let Some(rel_dir) = client.kind.project_skills_dir() else {
                        continue;
                    };
                    let _ = std::fs::remove_dir_all(workdir.join(rel_dir).join(slug));
                }
            }
        }

        // Clear selectors.matched, flocks.matched (leave manual_* untouched)
        let mut config = existing_config.clone();
        config.selectors.matched.clear();
        config.flocks.matched.clear();
        savhub_local::project::write_project_config_force(workdir, &config)?;

        // Clear savhub.lock (empty but file still exists)
        savhub_local::project::write_project_lockfile_force(
            workdir,
            &savhub_local::project::ProjectLockFile::default(),
        )?;

        if lockfile.skills.is_empty() {
            println!("No fetched skills to remove.");
        } else {
            println!(
                "\n\x1b[32mDone.\x1b[0m {} skill(s) removed.",
                lockfile.skills.len()
            );
        }

        return Ok(());
    }

    // ── Collect all matched items ──
    let matched_selector_names: Vec<String> = result
        .matched
        .iter()
        .map(|m| m.selector.name.clone())
        .collect();
    let matched_flocks: Vec<String> = result.flocks.iter().map(|s| s.to_string()).collect();

    // ── Collect previously matched selectors that no longer match ──
    let unmatched: Vec<tui::UnmatchedSelector> = existing_config
        .selectors
        .matched
        .iter()
        .filter(|prev| !matched_selector_names.contains(&prev.selector))
        .map(|prev| tui::UnmatchedSelector {
            name: prev.selector.clone(),
            flocks: prev.flocks.iter().map(|f| f.to_string()).collect(),
        })
        .collect();

    // ── Interactive selection of selectors and flocks (unless -y) ──
    let (selected_selectors, skipped_selectors): (Vec<String>, Vec<String>);
    let (selected_flocks, skipped_flocks): (Vec<String>, Vec<String>);

    if args.yes || !opts.input_allowed {
        selected_selectors = matched_selector_names.clone();
        skipped_selectors = Vec::new();
        selected_flocks = matched_flocks.clone();
        skipped_flocks = Vec::new();

        // Print summary
        if !selected_selectors.is_empty() {
            println!("\nSelectors:");
            for s in &selected_selectors {
                println!("  \x1b[32m[+]\x1b[0m {s}");
            }
        }
        if !selected_flocks.is_empty() {
            for (repo, members) in tui::group_flocks_by_repo(&selected_flocks) {
                println!("\nFlock {repo}");
                for f in &members {
                    println!("  \x1b[32m[+]\x1b[0m {}", tui::flock_display(f));
                }
            }
        }
        if !unmatched.is_empty() {
            println!("\n\x1b[33mWill be removed (no longer matched):\x1b[0m");
            for u in &unmatched {
                println!("  \x1b[31m✕\x1b[0m {}", u.name);
            }
        }
    } else {
        // Build TUI selectors with their contributed flocks
        let mut tui_selectors: Vec<tui::MatchedSelector> = result
            .matched
            .iter()
            .map(|m| {
                let pri = m.selector.priority;
                let sel_flocks: Vec<String> = m.flocks.iter().map(|s| s.to_string()).collect();
                tui::MatchedSelector {
                    name: m.selector.name.clone(),
                    label: format!("{} (P{pri}) — {}", m.selector.name, m.selector.description),
                    checked: !existing_config
                        .selectors
                        .manual_skipped
                        .contains(&m.selector.name),
                    flocks: sel_flocks,
                }
            })
            .collect();

        let flock_skip: BTreeSet<String> = existing_config
            .flocks
            .manual_skipped
            .iter()
            .cloned()
            .collect();

        // Pre-compute skill counts per flock to avoid API calls during TUI rendering.
        let flock_skill_counts: std::collections::HashMap<String, usize> = matched_flocks
            .iter()
            .map(|slug| {
                let flock_ref = savhub_local::selectors::SelectorSkillRef::parse(slug);
                let count = registry::list_flock_skills(&flock_ref.repo, &flock_ref.path)
                    .map(|v| v.len())
                    .unwrap_or(0);
                (slug.clone(), count)
            })
            .collect();

        let sel_result = tui::apply_select(
            &mut tui_selectors,
            &flock_skip,
            &flock_skill_counts,
            &unmatched,
        )?;

        let Some(sel) = sel_result else {
            println!("Cancelled.");
            return Ok(());
        };

        selected_selectors = sel.selected_selectors;
        skipped_selectors = sel.skipped_selectors;
        selected_flocks = sel.selected_flocks;
        skipped_flocks = sel.skipped_flocks;

        // Print summary after TUI
        if !selected_selectors.is_empty() || !skipped_selectors.is_empty() {
            println!("\nSelectors:");
            for s in &selected_selectors {
                println!("  \x1b[32m[+]\x1b[0m {s}");
            }
            for s in &skipped_selectors {
                println!("  \x1b[31m[-]\x1b[0m {s}");
            }
        }
        if !selected_flocks.is_empty() || !skipped_flocks.is_empty() {
            let all_flocks: Vec<String> = selected_flocks
                .iter()
                .chain(skipped_flocks.iter())
                .cloned()
                .collect();
            let selected_set: std::collections::HashSet<&String> = selected_flocks.iter().collect();
            for (repo, members) in tui::group_flocks_by_repo(&all_flocks) {
                println!("Flock {repo}");
                for f in &members {
                    if selected_set.contains(f) {
                        println!("  \x1b[32m[+]\x1b[0m {}", tui::flock_display(f));
                    } else {
                        println!("  \x1b[31m[-]\x1b[0m {}", tui::flock_display(f));
                    }
                }
            }
        }
    }

    // Merge CLI --skip-* args into skipped lists
    let mut skipped_flocks = skipped_flocks;
    for f in &args.skip_flocks {
        if !skipped_flocks.contains(f) {
            skipped_flocks.push(f.clone());
        }
    }

    // ── Expand selected flocks into skills (repo_url, skill_path) ──
    // Only use selectors that were selected (not skipped)
    // Track skills as (repo, path) for fetch, and slug for diff/display.
    let mut skill_map: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for m in &result.matched {
        if !selected_selectors.contains(&m.selector.name) {
            continue;
        }
        for skill in &m.skills {
            skill_map
                .entry(skill.to_string())
                .or_insert_with(|| (skill.repo.clone(), skill.path.clone()));
        }
    }
    for flock_slug in &selected_flocks {
        let flock_ref = savhub_local::selectors::SelectorSkillRef::parse(flock_slug);
        if let Ok(flock_skills) = registry::list_skills_in_flock(&flock_ref.repo, &flock_ref.path) {
            if flock_skills.is_empty() {
                eprintln!(
                    "  \x1b[33m!\x1b[0m flock \"{flock_slug}\" has 0 skills in the registry API"
                );
            }
            for skill in flock_skills {
                skill_map
                    .entry(skill.slug.clone())
                    .or_insert_with(|| (flock_ref.repo.clone(), skill.path.clone()));
            }
        }
    }

    // ── Include CLI --flocks skills ──
    for flock_slug in &args.add_flocks {
        let flock_ref = savhub_local::selectors::SelectorSkillRef::parse(flock_slug);
        if let Ok(flock_skills) = registry::list_skills_in_flock(&flock_ref.repo, &flock_ref.path) {
            for skill in flock_skills {
                skill_map
                    .entry(skill.slug.clone())
                    .or_insert_with(|| (flock_ref.repo.clone(), skill.path.clone()));
            }
        }
    }

    // ── Include CLI --skills directly ──
    for s in &args.add_skills {
        let skill_ref = savhub_local::selectors::SelectorSkillRef::parse(s);
        skill_map
            .entry(skill_ref.to_string())
            .or_insert_with(|| (skill_ref.repo.clone(), skill_ref.path.clone()));
    }

    // ── Preserve previously manual_added skills/flocks across runs.
    // Without this, a `savhub apply` on an unrelated change would drop any
    // entry the user had added with `savhub add` because the new desired set
    // is recomputed strictly from selectors + CLI args. ──
    for added in &existing_config.skills.manual_added {
        if added.slug.trim().is_empty() {
            continue;
        }
        let repo = added.repo.clone().unwrap_or_default();
        let path = if added.path.trim().is_empty() {
            added.slug.clone()
        } else {
            added.path.clone()
        };
        skill_map
            .entry(added.slug.clone())
            .or_insert((repo, path));
    }
    for flock_slug in &existing_config.flocks.manual_added {
        let flock_ref = savhub_local::selectors::SelectorSkillRef::parse(flock_slug);
        if let Ok(flock_skills) = registry::list_skills_in_flock(&flock_ref.repo, &flock_ref.path) {
            for skill in flock_skills {
                skill_map
                    .entry(skill.slug.clone())
                    .or_insert_with(|| (flock_ref.repo.clone(), skill.path.clone()));
            }
        }
    }

    // ── Filter out skipped skills (existing config + CLI --skip-skills) ──
    let mut skipped = existing_config.skills.manual_skipped.clone();
    for s in &args.skip_skills {
        if !s.is_empty() && !skipped.contains(s) {
            skipped.push(s.clone());
        }
    }
    let skipped = &skipped;
    let desired_skills: BTreeSet<String> = skill_map
        .keys()
        .filter(|s| !registry::skill_matches_skipped(s, skipped))
        .cloned()
        .collect();

    // ── Compute diff against current lockfile ──
    let current_lock = savhub_local::project::read_project_lockfile(workdir)?;
    let current_locked_slugs: BTreeSet<String> = current_lock
        .skills
        .iter()
        .map(|s| s.slug.as_str().to_string())
        .collect();

    let mut to_add: Vec<String> = desired_skills
        .difference(&current_locked_slugs)
        .cloned()
        .collect();
    let to_remove: Vec<String> = current_locked_slugs
        .difference(&desired_skills)
        .cloned()
        .collect();

    // ── Also restore skills that are in the lock but missing from disk ──
    {
        let check_clients: Vec<_> = savhub_local::clients::detect_clients()
            .into_iter()
            .filter(|c| {
                let name = c.kind.as_str();
                if !args.agents.is_empty() {
                    return args.agents.iter().any(|a| a.eq_ignore_ascii_case(name));
                }
                if !args.skip_agents.is_empty() {
                    return !args
                        .skip_agents
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(name));
                }
                true
            })
            .filter(|c| c.installed && c.kind.project_skills_dir().is_some())
            .collect();

        for slug in desired_skills.intersection(&current_locked_slugs) {
            let missing_from_disk = check_clients.iter().any(|c| {
                let rel = c.kind.project_skills_dir().unwrap();
                !workdir.join(rel).join(slug).exists()
            });
            if missing_from_disk && !to_add.contains(slug) {
                to_add.push(slug.clone());
            }
        }
    }

    // ── Check if anything actually changed ──
    let toml_exists = workdir.join("savhub.toml").exists();
    let lock_exists = workdir.join("savhub.lock").exists();
    if to_add.is_empty() && to_remove.is_empty() && toml_exists && lock_exists {
        // Also check if selectors/flocks config changed
        let old_matched_names: BTreeSet<String> = existing_config
            .selectors
            .matched
            .iter()
            .map(|m| m.selector.clone())
            .collect();
        let new_matched_names: BTreeSet<String> = result
            .matched
            .iter()
            .map(|m| m.selector.name.clone())
            .collect();
        let old_flocks: BTreeSet<String> = existing_config
            .flocks
            .matched
            .iter()
            .map(|r| r.to_string())
            .collect();
        let new_flocks: BTreeSet<String> = selected_flocks.iter().cloned().collect();
        if old_matched_names == new_matched_names && old_flocks == new_flocks {
            println!("\nProject is already up to date. Nothing to do.");
            return Ok(());
        }
    }

    // ── Show plan (compact summary) ──
    if !to_add.is_empty() || !to_remove.is_empty() {
        println!(
            "\nChanges: \x1b[32m+{}\x1b[0m skill(s) to add, \x1b[31m-{}\x1b[0m to remove",
            to_add.len(),
            to_remove.len()
        );
    } else {
        println!("\nNo skill changes, updating selector configuration only.");
    }

    if args.dry_run {
        if !to_add.is_empty() {
            println!("\nSkills to add:");
            for s in &to_add {
                println!("  \x1b[32m[+]\x1b[0m {s}");
            }
        }
        if !to_remove.is_empty() {
            println!("Skills to remove:");
            for s in &to_remove {
                println!("  \x1b[31m[-]\x1b[0m {s}");
            }
        }
        println!("\n\x1b[2m(dry-run: no changes made)\x1b[0m");
        return Ok(());
    }

    if !args.yes && opts.input_allowed && (!to_add.is_empty() || !to_remove.is_empty()) {
        let proceed = Confirm::new()
            .with_prompt("Apply these changes?")
            .default(true)
            .interact()?;
        if !proceed {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // ── Apply: update savhub.toml selectors (replace, not accumulate) ──
    {
        let mut cfg = savhub_local::project::read_project_config(workdir)?;
        cfg.selectors.matched = result
            .matched
            .iter()
            .map(|m| {
                let selector_flocks: Vec<savhub_local::selectors::SelectorSkillRef> = m
                    .flocks
                    .iter()
                    .filter(|f| selected_flocks.contains(&f.to_string()))
                    .cloned()
                    .collect();
                savhub_local::project::ProjectSelectorMatch {
                    selector: m.selector.name.clone(),
                    flocks: selector_flocks,
                    skills: m.skills.clone(),
                    repos: m.repos.clone(),
                }
            })
            .collect();
        // Collect all matched flocks into flocks.matched
        let mut all_matched_flocks: Vec<savhub_local::selectors::SelectorSkillRef> = Vec::new();
        for m in &cfg.selectors.matched {
            for f in &m.flocks {
                if !all_matched_flocks.contains(f) {
                    all_matched_flocks.push(f.clone());
                }
            }
        }
        cfg.flocks.matched = all_matched_flocks;

        // Save interactive unchecked items to manual_skipped
        for s in &skipped_selectors {
            if !cfg.selectors.manual_skipped.contains(s) {
                cfg.selectors.manual_skipped.push(s.clone());
            }
        }
        // Remove re-checked items from manual_skipped
        cfg.selectors
            .manual_skipped
            .retain(|s| !selected_selectors.contains(s) || !matched_selector_names.contains(s));

        for f in &skipped_flocks {
            if !cfg.flocks.manual_skipped.contains(f) {
                cfg.flocks.manual_skipped.push(f.clone());
            }
        }
        cfg.flocks
            .manual_skipped
            .retain(|f| !selected_flocks.contains(f) || !matched_flocks.contains(f));

        // Merge CLI --skills/--skip-skills/--flocks/--skip-flocks
        for s in &args.add_skills {
            if !s.is_empty() && !cfg.skills.manual_added.iter().any(|e| e.path == *s) {
                cfg.skills
                    .manual_added
                    .push(savhub_local::project::ProjectAddedSkill {
                        path: s.rsplit('/').next().unwrap_or(s).to_string(),
                        slug: s.rsplit('/').next().unwrap_or(s).to_string(),
                        repo: None,
                        local: None,
                        version: None,
                        fetched_at: 0,
                    });
            }
        }
        for s in &args.skip_skills {
            if !s.is_empty() && !cfg.skills.manual_skipped.contains(s) {
                cfg.skills.manual_skipped.push(s.clone());
            }
        }
        for f in &args.add_flocks {
            if !f.is_empty() && !cfg.flocks.manual_added.contains(f) {
                cfg.flocks.manual_added.push(f.clone());
            }
        }
        for f in &args.skip_flocks {
            if !f.is_empty() && !cfg.flocks.manual_skipped.contains(f) {
                cfg.flocks.manual_skipped.push(f.clone());
            }
        }

        savhub_local::project::write_project_config_force(workdir, &cfg)?;
    }

    // ── Update selector match counts ──
    {
        let unmatched_names: Vec<String> = unmatched.iter().map(|u| u.name.clone()).collect();
        let _ =
            savhub_local::selectors::update_match_counts(&matched_selector_names, &unmatched_names);
    }

    // ── Remove skills that are no longer in desired set (grouped by repo) ──
    if !to_remove.is_empty() {
        let all_clients = savhub_local::clients::detect_clients();
        // Group by repo from current lock
        for slug in &to_remove {
            for client in &all_clients {
                if !client.installed {
                    continue;
                }
                let Some(rel_dir) = client.kind.project_skills_dir() else {
                    continue;
                };
                let _ = std::fs::remove_dir_all(workdir.join(rel_dir).join(slug));
            }
            println!("  \x1b[31m\u{2717}\x1b[0m {slug} (removed)");
        }
    }

    // ── Apply: batch-fetch skills via registry (one git op per repo) ──
    use savhub_local::skills::copy_skill_folder;

    let mut fetched_count = 0usize;

    // Filter AI clients (respecting --agents/--skip-agents)
    let all_clients = savhub_local::clients::detect_clients();
    let filtered_clients: Vec<_> = all_clients
        .into_iter()
        .filter(|c| {
            let name = c.kind.as_str();
            if !args.agents.is_empty() {
                return args.agents.iter().any(|a| a.eq_ignore_ascii_case(name));
            }
            if !args.skip_agents.is_empty() {
                return !args
                    .skip_agents
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(name));
            }
            true
        })
        .collect();

    let to_add_pairs: Vec<(String, String)> = to_add
        .iter()
        .filter_map(|slug| skill_map.get(slug).cloned())
        .collect();

    // Build lock entries: start from current, remove deleted, add new
    let mut lock = current_lock.clone();
    lock.skills
        .retain(|s| !to_remove.iter().any(|r| r == s.slug.as_str()));

    // Fetch with per-skill progress output (each skill appends a line)
    let batch_results =
        registry::fetch_skills_batch_with_progress(&to_add_pairs, |_idx, _total, result| {
            match result {
                Ok(slug) => {
                    eprintln!("  \x1b[32m\u{2713}\x1b[0m {slug}");
                }
                Err(label) => {
                    eprintln!("  \x1b[31m\u{2717}\x1b[0m {label}");
                }
            }
        })?;

    // Copy fetched skills to AI client directories
    {
        for info in &batch_results {
            let mut client_names = Vec::new();
            for client in &filtered_clients {
                if !client.installed {
                    continue;
                }
                let Some(rel_dir) = client.kind.project_skills_dir() else {
                    continue;
                };
                let target_dir = workdir.join(rel_dir);
                let _ = std::fs::create_dir_all(&target_dir);
                let target = target_dir.join(&info.slug);
                if let Err(e) = copy_skill_folder(&info.local_path, &target) {
                    eprintln!(
                        "  \x1b[33m!\x1b[0m {}: failed to copy to {}: {e}",
                        info.slug, rel_dir
                    );
                    continue;
                }
                client_names.push(client.kind.as_str());
            }

            // Record in savhub.lock
            if !lock.skills.iter().any(|s| s.slug.as_str() == info.slug) {
                let vi = savhub_local::skills::read_skill_version_info(&info.local_path)
                    .unwrap_or_default();
                lock.skills.push(savhub_local::project::ProjectLockedSkill {
                    repo: Some(info.repo_sign.clone()),
                    path: Some(info.skill_path.clone()),
                    slug: info.slug.clone(),
                    version: vi.version,
                    git_sha: vi.git_sha,
                });
            }
            fetched_count += 1;
        }
    }

    // Always create savhub.lock (even if empty)
    savhub_local::project::write_project_lockfile_force(workdir, &lock)?;

    // Register this project so desktop can see it
    let _ = savhub_local::config::add_project(&workdir.display().to_string());

    // Fire-and-forget install tracking
    if !batch_results.is_empty()
        && let Ok(client) = optional_client(opts)
        && let Ok(handle) = tokio::runtime::Handle::try_current()
    {
        for info in &batch_results {
            let slug = info.slug.clone();
            let client = client.clone();
            handle.spawn(async move {
                let _ = client
                    .post_json::<serde_json::Value, serde_json::Value>(
                        &format!("/collect?skill={slug}"),
                        &json!({ "client_type": "cli" }),
                    )
                    .await;
            });
        }
    }

    let removed_count = to_remove.len();
    if fetched_count > 0 || removed_count > 0 {
        println!(
            "\n\x1b[32mDone.\x1b[0m +{fetched_count} -{removed_count} skill(s), {} selector(s) matched.",
            result.matched.len()
        );
    } else {
        println!(
            "\n\x1b[32mDone.\x1b[0m Configuration updated, {} selector(s) matched.",
            result.matched.len()
        );
    }
    Ok(())
}
