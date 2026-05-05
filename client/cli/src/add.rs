//! `savhub add` — manually add a skill or flock to the current project's
//! `manual_added` lists in `savhub.toml`. Manual entries survive future
//! `savhub apply` runs even when no selector matches them.
//!
//! Resolution accepts a partial name: when more than one registry entry
//! matches, the user is asked to pick which one.

use anyhow::{Result, anyhow, bail};
use clap::{ArgAction, Args, Subcommand};
use dialoguer::{Confirm, Select};
use savhub_local::project::{
    ProjectAddedSkill, read_project_config, write_project_config_force,
};
use savhub_local::registry::{self, FetchedSkillInfo, SkillSearchEntry};
use savhub_local::selectors::SelectorSkillRef;
use savhub_local::skills::copy_skill_folder;
use savhub_shared::RegistryFlock;

use crate::GlobalOpts;

#[derive(Debug, Subcommand)]
pub(crate) enum AddCommand {
    /// Add a skill to this project's manual_added list (survives `savhub apply`)
    Skill(AddSkillArgs),
    /// Add a flock to this project's manual_added list (survives `savhub apply`)
    Flock(AddFlockArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AddSkillArgs {
    /// Skill slug or partial name (interactive disambiguation when ambiguous)
    pub name: String,
    /// Skip confirmation prompts
    #[arg(long, action = ArgAction::SetTrue)]
    pub yes: bool,
    /// Search-result cap when disambiguating a partial name
    #[arg(long, default_value_t = 25)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub(crate) struct AddFlockArgs {
    /// Flock slug or partial name (interactive disambiguation when ambiguous)
    pub name: String,
    /// Skip confirmation prompts
    #[arg(long, action = ArgAction::SetTrue)]
    pub yes: bool,
    /// Search-result cap when disambiguating a partial name
    #[arg(long, default_value_t = 25)]
    pub limit: usize,
}

pub(crate) fn cmd_add_skill(opts: &GlobalOpts, args: AddSkillArgs) -> Result<()> {
    let query = args.name.trim();
    if query.is_empty() {
        bail!("skill name required");
    }

    let candidates = registry::search_skill_entries(query, args.limit.max(1))?;
    let chosen = pick_skill(opts, query, &candidates, args.yes)?;

    let repo_url = chosen.repo_url.trim();
    if repo_url.is_empty() {
        bail!(
            "registry returned no repo_url for skill `{}` (server data may be stale)",
            chosen.slug
        );
    }
    let skill_path = if chosen.path.trim().is_empty() {
        chosen.slug.clone()
    } else {
        chosen.path.clone()
    };

    if !args.yes && opts.input_allowed {
        let proceed = Confirm::new()
            .with_prompt(format!(
                "Add skill `{}` from {}?",
                chosen.slug, repo_url
            ))
            .default(true)
            .interact()
            .map_err(|error| anyhow!("failed to read confirmation: {error}"))?;
        if !proceed {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let mut config = read_project_config(&opts.workdir)?;
    let already = config
        .skills
        .manual_added
        .iter()
        .any(|entry| entry.path == skill_path || entry.slug == chosen.slug);
    if !already {
        config.skills.manual_added.push(ProjectAddedSkill {
            path: skill_path.clone(),
            slug: chosen.slug.clone(),
            repo: Some(repo_url.to_string()),
            local: None,
            version: chosen.version.clone(),
            fetched_at: 0,
        });
    }
    config
        .skills
        .manual_skipped
        .retain(|entry| entry != &chosen.slug && entry != &skill_path);
    write_project_config_force(&opts.workdir, &config)?;

    let fetched = registry::fetch_skills_batch(&[(repo_url.to_string(), skill_path.clone())])?;
    if fetched.is_empty() {
        bail!("registry fetch produced no skill bundle for `{}`", chosen.slug);
    }
    install_skills_into_project(&opts.workdir, &fetched)?;

    let label = if already { "already added" } else { "added" };
    println!(
        "Skill `{}` {label} ({} skill bundle{}).",
        chosen.slug,
        fetched.len(),
        if fetched.len() == 1 { "" } else { "s" }
    );
    Ok(())
}

pub(crate) fn cmd_add_flock(opts: &GlobalOpts, args: AddFlockArgs) -> Result<()> {
    let query = args.name.trim();
    if query.is_empty() {
        bail!("flock name required");
    }

    let candidates = registry::search_flocks(query, args.limit.max(1))?;
    let chosen = pick_flock(opts, query, &candidates, args.yes)?;

    if !args.yes && opts.input_allowed {
        let proceed = Confirm::new()
            .with_prompt(format!(
                "Add flock `{}` from {}?",
                chosen.slug, chosen.repo
            ))
            .default(true)
            .interact()
            .map_err(|error| anyhow!("failed to read confirmation: {error}"))?;
        if !proceed {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let flock_ref = SelectorSkillRef {
        repo: chosen.repo.clone(),
        path: chosen.slug.clone(),
    };
    let flock_token = flock_ref.to_string();

    let mut config = read_project_config(&opts.workdir)?;
    let already = config.flocks.manual_added.iter().any(|f| f == &flock_token);
    if !already {
        config.flocks.manual_added.push(flock_token.clone());
    }
    config
        .flocks
        .manual_skipped
        .retain(|entry| entry != &flock_token && entry != &chosen.slug);
    write_project_config_force(&opts.workdir, &config)?;

    let pairs: Vec<(String, String)> = registry::list_skills_in_flock(&chosen.repo, &chosen.slug)?
        .into_iter()
        .map(|skill| (chosen.repo.clone(), skill.path))
        .collect();
    let total = pairs.len();
    let fetched = if pairs.is_empty() {
        Vec::new()
    } else {
        registry::fetch_skills_batch(&pairs)?
    };
    install_skills_into_project(&opts.workdir, &fetched)?;

    let label = if already { "already added" } else { "added" };
    println!(
        "Flock `{}` {label} ({} skill bundle{} fetched of {}).",
        chosen.slug,
        fetched.len(),
        if fetched.len() == 1 { "" } else { "s" },
        total
    );
    Ok(())
}

fn pick_skill(
    opts: &GlobalOpts,
    query: &str,
    candidates: &[SkillSearchEntry],
    non_interactive_ok: bool,
) -> Result<SkillSearchEntry> {
    if candidates.is_empty() {
        bail!("no skill matches `{}`", query);
    }
    let exact: Vec<&SkillSearchEntry> = candidates
        .iter()
        .filter(|c| c.slug.eq_ignore_ascii_case(query) || c.name.eq_ignore_ascii_case(query))
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    if candidates.len() == 1 {
        return Ok(candidates[0].clone());
    }
    if !opts.input_allowed || non_interactive_ok {
        let listing = candidates
            .iter()
            .map(|c| format!("  {} — {}", c.slug, c.name))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "multiple skills match `{}`; pass an exact slug:\n{}",
            query,
            listing
        );
    }
    let labels: Vec<String> = candidates
        .iter()
        .map(|c| {
            let summary = c
                .description
                .as_deref()
                .map(|s| s.chars().take(60).collect::<String>())
                .unwrap_or_default();
            if summary.is_empty() {
                format!("{:<32}  {}", c.slug, c.name)
            } else {
                format!("{:<32}  {} — {}", c.slug, c.name, summary)
            }
        })
        .collect();
    let pick = Select::new()
        .with_prompt(format!("Pick skill matching `{query}`"))
        .items(&labels)
        .default(0)
        .interact_opt()
        .map_err(|error| anyhow!("failed to read selection: {error}"))?
        .ok_or_else(|| anyhow!("cancelled"))?;
    Ok(candidates[pick].clone())
}

fn pick_flock(
    opts: &GlobalOpts,
    query: &str,
    candidates: &[RegistryFlock],
    non_interactive_ok: bool,
) -> Result<RegistryFlock> {
    if candidates.is_empty() {
        bail!("no flock matches `{}`", query);
    }
    let exact: Vec<&RegistryFlock> = candidates
        .iter()
        .filter(|c| c.slug.eq_ignore_ascii_case(query) || c.name.eq_ignore_ascii_case(query))
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    if candidates.len() == 1 {
        return Ok(candidates[0].clone());
    }
    if !opts.input_allowed || non_interactive_ok {
        let listing = candidates
            .iter()
            .map(|c| format!("  {} — {}", c.slug, c.name))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "multiple flocks match `{}`; pass an exact slug:\n{}",
            query,
            listing
        );
    }
    let labels: Vec<String> = candidates
        .iter()
        .map(|c| {
            let summary = c.description.chars().take(60).collect::<String>();
            if summary.is_empty() {
                format!("{:<32}  {}", c.slug, c.name)
            } else {
                format!("{:<32}  {} — {}", c.slug, c.name, summary)
            }
        })
        .collect();
    let pick = Select::new()
        .with_prompt(format!("Pick flock matching `{query}`"))
        .items(&labels)
        .default(0)
        .interact_opt()
        .map_err(|error| anyhow!("failed to read selection: {error}"))?
        .ok_or_else(|| anyhow!("cancelled"))?;
    Ok(candidates[pick].clone())
}

/// Copy the freshly-fetched skills into each detected AI client's project
/// skills directory and refresh `savhub.lock`. Mirrors `apply.rs` so a manual
/// `add` lands the skills on disk without a follow-up `apply`.
fn install_skills_into_project(workdir: &std::path::Path, fetched: &[FetchedSkillInfo]) -> Result<()> {
    if fetched.is_empty() {
        return Ok(());
    }
    let clients = savhub_local::clients::detect_clients();
    for info in fetched {
        for client in &clients {
            if !client.installed {
                continue;
            }
            let Some(rel_dir) = client.kind.project_skills_dir() else {
                continue;
            };
            let target_dir = workdir.join(rel_dir);
            std::fs::create_dir_all(&target_dir).ok();
            let target = target_dir.join(&info.slug);
            if let Err(e) = copy_skill_folder(&info.local_path, &target) {
                eprintln!(
                    "  ! {}: failed to copy to {}: {e}",
                    info.slug, rel_dir
                );
            }
        }
        println!("  + {}", info.slug);
    }

    let mut lock = savhub_local::project::read_project_lockfile(workdir)?;
    for info in fetched {
        let vi =
            savhub_local::skills::read_skill_version_info(&info.local_path).unwrap_or_default();
        if let Some(existing) = lock.skills.iter_mut().find(|s| s.slug == info.slug) {
            existing.repo = Some(info.repo_sign.clone());
            existing.path = Some(info.skill_path.clone());
            existing.version = vi.version.clone();
            existing.git_sha = vi.git_sha.clone();
        } else {
            lock.skills.push(savhub_local::project::ProjectLockedSkill {
                repo: Some(info.repo_sign.clone()),
                path: Some(info.skill_path.clone()),
                slug: info.slug.clone(),
                version: vi.version,
                git_sha: vi.git_sha,
            });
        }
    }
    savhub_local::project::write_project_lockfile_force(workdir, &lock)?;

    let _ = savhub_local::config::add_project(&workdir.display().to_string());
    Ok(())
}
