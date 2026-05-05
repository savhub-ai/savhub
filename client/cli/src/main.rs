mod add;
mod admin;
mod apply;
mod auth;
mod inspect;
mod oauth;
mod tui;
mod updater;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use clap::{ArgAction, Args, Parser, Subcommand};
use dialoguer::Confirm;
use savhub_local::api::ApiClient;
use savhub_local::config::read_global_config;
use savhub_local::project::{
    disable_project_skill, enable_repo_skill_in_project, read_project_added_skills,
    write_project_added_skills,
};
use savhub_local::registry::{fetch_version_label, install_remote_skill_from_repo};
use savhub_local::skills::{
    LockSkill, RepoSkillOrigin, read_lockfile, write_repo_skill_origin,
};
use savhub_shared::{
    PagedResponse, RemoteSkillFetchSpec, RepoDetailResponse, SearchResponse, SkillDetailResponse,
    SkillListItem,
};

const DEFAULT_SITE: &str = "https://savhub.ai";

fn exe_location() -> String {
    let path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    format!("Binary: {path}")
}

#[derive(Debug, Parser)]
#[command(
    name = "savhub",
    version = savhub_local::build_info::VERSION_LONG,
    about = "Savhub CLI\n\nDocumentation: https://savhub.ai/en/docs/client",
    after_help = exe_location(),
)]
struct Cli {
    /// Config/data directory (overrides SAVHUB_CONFIG_DIR and ~/.savhub)
    #[arg(long, global = true)]
    profile: Option<PathBuf>,
    #[arg(long, global = true)]
    workdir: Option<PathBuf>,
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[arg(long, global = true)]
    site: Option<String>,
    #[arg(long, global = true)]
    api_base: Option<String>,
    #[arg(long = "no-input", global = true, action = ArgAction::SetTrue)]
    no_input: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Login via GitHub OAuth
    Login(LoginArgs),
    /// Clear local auth token
    Logout,
    /// Show current authenticated user
    Whoami,
    /// Search skills in the registry
    Search(SearchArgs),
    /// Enable a skill from a local repo into a project
    Enable(EnableArgs),
    /// Disable a skill in the current project
    Disable(DisableArgs),
    /// Fetch a skill by cloning its source repo
    Fetch(FetchArgs),
    /// Update all project skills from fetched repo cache
    Update(UpdateArgs),
    /// List or update fetched skills from ~/.savhub/fetched.json
    Fetched(FetchedArgs),
    /// Prune a skill
    Prune(PruneArgs),
    /// List fetched skills in the current project
    List,
    /// Browse skills from the registry API
    Explore(ExploreArgs),
    /// View detailed info about a skill
    Inspect(InspectArgs),
    /// Manage registry access
    Registry {
        #[command(subcommand)]
        command: RegistryCommand,
    },
    /// Manage selectors (project type detection rules)
    Selector {
        #[command(subcommand)]
        command: SelectorCommand,
    },
    /// Detect project type via selectors and apply skills to AI clients
    Apply(ApplyArgs),
    /// Manually add a skill or flock to this project (preserved across `apply`)
    Add {
        #[command(subcommand)]
        command: add::AddCommand,
    },
    /// Manage flocks (skill collections)
    Flock {
        #[command(subcommand)]
        command: FlockCommand,
    },
    /// Manage bundled AI skills (install/uninstall/status)
    Pilot {
        #[command(subcommand)]
        command: PilotCommand,
    },
    /// Open documentation in the browser
    Docs,
    /// Update the savhub CLI to the latest version
    SelfUpdate,
}

#[derive(Debug, Subcommand)]
enum SelectorCommand {
    /// List all configured selectors
    List,
    /// Show details of a selector by name
    Show(SelectorShowArgs),
    /// Run all selectors against a directory and show matches (no changes)
    Test,
}

#[derive(Debug, Args)]
struct SelectorShowArgs {
    /// Selector name (partial match)
    name: String,
}

#[derive(Debug, Subcommand)]
enum RegistryCommand {
    /// Search registry skills
    Search(RegistrySearchArgs),
    /// List registry skills with pagination
    List(RegistryListArgs),
}

#[derive(Debug, Args)]
struct RegistrySearchArgs {
    query: Vec<String>,
    #[arg(long, default_value_t = 25)]
    limit: usize,
}

#[derive(Debug, Args)]
struct RegistryListArgs {
    #[arg(long, default_value_t = 1)]
    page: usize,
    #[arg(long, default_value_t = 25)]
    page_size: usize,
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    status: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum FlockCommand {
    /// List all flocks from the registry cache
    List,
    /// Show details of a flock and its skills
    Show(FlockShowArgs),
    /// Fetch all skills from a flock
    Fetch(FlockFetchArgs),
}

#[derive(Debug, Args)]
struct FlockShowArgs {
    /// Flock slug
    slug: String,
}

#[derive(Debug, Args)]
struct FlockFetchArgs {
    /// Flock slug
    slug: String,
    /// Skip confirmation prompt
    #[arg(long, action = ArgAction::SetTrue)]
    yes: bool,
}

#[derive(Debug, Subcommand)]
enum PilotCommand {
    /// Install bundled skills into AI agent skill directories
    Install(PilotAgentArgs),
    /// Uninstall bundled skills from AI agent skill directories
    Uninstall(PilotAgentArgs),
    /// Show installation status for each configured agent
    Status(PilotAgentArgs),
    /// Touch the config-changed signal file (useful for external tools)
    Notify,
}

#[derive(Debug, Args)]
struct PilotAgentArgs {
    /// Override which agents to target (defaults to agents from config)
    #[arg(long, num_args = 1.., value_delimiter = ',')]
    agents: Vec<String>,
}

#[derive(Debug, Default, Clone, Args)]
pub(crate) struct ApplyArgs {
    /// Show what would be done without making changes
    #[arg(long = "dry-run", action = ArgAction::SetTrue)]
    pub dry_run: bool,
    /// Skip confirmation prompt
    #[arg(long, action = ArgAction::SetTrue)]
    pub yes: bool,
    /// Only sync skills to these AI agents (e.g. --agents claude-code codex)
    #[arg(long, num_args = 1.., value_delimiter = ',')]
    pub agents: Vec<String>,
    /// Skip syncing skills to these AI agents (e.g. --skip-agents cursor windsurf)
    #[arg(long = "skip-agents", num_args = 1.., value_delimiter = ',')]
    pub skip_agents: Vec<String>,
    /// Manually add skills by slug or sign (saved to savhub.toml skills.manual_added)
    #[arg(long = "skills", num_args = 1.., value_delimiter = ',')]
    pub add_skills: Vec<String>,
    /// Manually skip skills by slug or sign (saved to savhub.toml skills.manual_skipped)
    #[arg(long = "skip-skills", num_args = 1.., value_delimiter = ',')]
    pub skip_skills: Vec<String>,
    /// Manually add flocks (saved to savhub.toml flocks.manual_added)
    #[arg(long = "flocks", num_args = 1.., value_delimiter = ',')]
    pub add_flocks: Vec<String>,
    /// Manually skip flocks (saved to savhub.toml flocks.manual_skipped)
    #[arg(long = "skip-flocks", num_args = 1.., value_delimiter = ',')]
    pub skip_flocks: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct LoginArgs {
    #[arg(long, hide = true)]
    pub token: Option<String>,
    #[arg(long, hide = true)]
    pub label: Option<String>,
    #[arg(long = "no-browser", action = ArgAction::SetTrue)]
    pub no_browser: bool,
}

#[derive(Debug, Args)]
struct SearchArgs {
    query: Vec<String>,
    #[arg(long)]
    limit: Option<i64>,
}

#[derive(Debug, Args)]
struct EnableArgs {
    slug: String,
    #[arg(long)]
    repo: String,
    #[arg(long = "selector", alias = "detector")]
    selectors: Vec<String>,
}

#[derive(Debug, Args)]
struct DisableArgs {
    slug: String,
    #[arg(long, action = ArgAction::SetTrue)]
    yes: bool,
}

#[derive(Debug, Args)]
struct FetchArgs {
    slug: String,
    #[arg(long)]
    version: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    force: bool,
}

#[derive(Debug, Args)]
struct UpdateArgs {}

#[derive(Debug, Args)]
struct FetchedArgs {
    /// Update all fetched repos and skills to the latest version from the registry
    #[arg(long, action = ArgAction::SetTrue)]
    update: bool,
    /// Remove repos/flocks/skills not used by any project
    #[arg(long, action = ArgAction::SetTrue)]
    prune: bool,
    /// Force update even if already at latest
    #[arg(long, action = ArgAction::SetTrue)]
    force: bool,
}

#[derive(Debug, Args)]
struct PruneArgs {
    slug: String,
    #[arg(long, action = ArgAction::SetTrue)]
    yes: bool,
}

#[derive(Debug, Args)]
struct ExploreArgs {
    #[arg(long, default_value_t = 25)]
    limit: i64,
    #[arg(long, default_value = "newest")]
    sort: String,
    #[arg(long, action = ArgAction::SetTrue)]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct InspectArgs {
    pub slug: String,
    #[arg(long)]
    pub version: Option<String>,
    #[arg(long)]
    pub tag: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub versions: bool,
    #[arg(long)]
    pub limit: Option<usize>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub files: bool,
    #[arg(long)]
    pub file: Option<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DeleteArgs {
    pub slug: String,
    #[arg(long, action = ArgAction::SetTrue)]
    pub yes: bool,
}

// The Args structs below stay declared here so clap can derive them from a
// single Cli-rooted definition, but they are consumed by handlers that now
// live in `admin.rs` (see that module for the dormant-command rationale).
#[derive(Debug, Args)]
pub(crate) struct PublishArgs {
    pub path: PathBuf,
    #[arg(long)]
    pub slug: Option<String>,
    #[arg(long = "name")]
    pub display_name: Option<String>,
    #[arg(long)]
    pub version: Option<String>,
    #[arg(long)]
    pub changelog: Option<String>,
    #[arg(long, default_value = "latest")]
    pub tags: String,
}
#[derive(Debug, Args)]
pub(crate) struct SyncArgs {
    #[arg(long = "root")]
    pub roots: Vec<PathBuf>,
    #[arg(long, action=ArgAction::SetTrue)]
    pub all: bool,
    #[arg(long="dry-run", action=ArgAction::SetTrue)]
    pub dry_run: bool,
    #[arg(long, default_value = "patch")]
    pub bump: String,
    #[arg(long)]
    pub changelog: Option<String>,
    #[arg(long, default_value = "latest")]
    pub tags: String,
    #[arg(long, default_value_t = 4)]
    pub concurrency: usize,
}
#[derive(Debug, Args)]
pub(crate) struct BanUserArgs {
    pub handle_or_id: String,
    #[arg(long, action=ArgAction::SetTrue)]
    pub id: bool,
    #[arg(long, action=ArgAction::SetTrue)]
    pub fuzzy: bool,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long, action=ArgAction::SetTrue)]
    pub yes: bool,
}
#[derive(Debug, Args)]
pub(crate) struct SetRoleArgs {
    pub handle_or_id: String,
    pub role: String,
    #[arg(long, action=ArgAction::SetTrue)]
    pub id: bool,
    #[arg(long, action=ArgAction::SetTrue)]
    pub fuzzy: bool,
    #[arg(long, action=ArgAction::SetTrue)]
    pub yes: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct GlobalOpts {
    pub workdir: PathBuf,
    pub dir: PathBuf,
    pub api_base: String,
    pub input_allowed: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(profile) = &cli.profile {
        let resolved = if profile.is_absolute() {
            profile.clone()
        } else {
            // Resolve relative paths (including ~ and bare names like .savhub-dev)
            // against the user's home directory, never the project working directory.
            let stripped = profile.strip_prefix("~").unwrap_or(profile);
            directories::UserDirs::new()
                .map(|d| d.home_dir().join(stripped))
                .unwrap_or_else(|| profile.clone())
        };
        // SAFETY: called before any threads are spawned.
        unsafe { std::env::set_var("SAVHUB_CONFIG_DIR", resolved) };
    }

    // Clean up backup binary from a previous self-update
    updater::cleanup_old_binary();

    let opts = resolve_global_opts(&cli)?;

    match cli.command {
        Some(Command::Login(args)) => auth::cmd_login(&opts, args).await?,
        Some(Command::Logout) => auth::cmd_logout(&opts)?,
        Some(Command::Whoami) => auth::cmd_whoami(&opts).await?,
        Some(Command::Search(args)) => cmd_search(&opts, args).await?,
        Some(Command::Enable(args)) => cmd_enable(&opts, args)?,
        Some(Command::Disable(args)) => cmd_disable(&opts, args)?,
        Some(Command::Fetch(args)) => cmd_fetch(&opts, args).await?,
        Some(Command::Update(args)) => cmd_update(&opts, args)?,
        Some(Command::Fetched(args)) => cmd_fetched(&opts, args).await?,
        Some(Command::Prune(args)) => cmd_prune(&opts, args)?,
        Some(Command::List) => cmd_list(&opts)?,
        Some(Command::Explore(args)) => cmd_explore(&opts, args).await?,
        Some(Command::Inspect(args)) => inspect::cmd_inspect(&opts, args).await?,
        Some(Command::Registry { command }) => cmd_registry(&opts, command).await?,
        Some(Command::Selector { command }) => cmd_selector(&opts, command)?,
        Some(Command::Apply(args)) => {
            let opts = opts.clone();
            tokio::task::spawn_blocking(move || apply::cmd_apply(&opts, args))
                .await
                .context("apply task panicked")??;
        }
        Some(Command::Add { command }) => {
            let opts = opts.clone();
            tokio::task::spawn_blocking(move || match command {
                add::AddCommand::Skill(args) => add::cmd_add_skill(&opts, args),
                add::AddCommand::Flock(args) => add::cmd_add_flock(&opts, args),
            })
            .await
            .context("add task panicked")??;
        }
        Some(Command::Flock { command }) => cmd_flock(&opts, command)?,
        Some(Command::Pilot { command }) => cmd_pilot(command)?,
        Some(Command::Docs) => {
            let url = "https://savhub.ai/en/docs/client";
            println!("Documentation: {url}");
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", "", url])
                    .spawn();
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open").arg(url).spawn();
            }
            #[cfg(target_os = "linux")]
            {
                let _ = std::process::Command::new("xdg-open").arg(url).spawn();
            }
        }
        Some(Command::SelfUpdate) => updater::run_self_update().await?,
        None => {
            let opts = opts.clone();
            tokio::task::spawn_blocking(move || apply::cmd_apply(&opts, ApplyArgs::default()))
                .await
                .context("apply task panicked")??;
        }
    }

    Ok(())
}

fn resolve_global_opts(cli: &Cli) -> Result<GlobalOpts> {
    let workdir = resolve_workdir(cli)?;
    let dir = workdir.join(
        cli.dir
            .clone()
            .or_else(|| std::env::var_os("SAVHUB_DIR").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("skills")),
    );
    let site = cli
        .site
        .clone()
        .or_else(|| std::env::var("SAVHUB_SITE").ok())
        .unwrap_or_else(|| DEFAULT_SITE.to_string());
    // Priority: --api-base flag > env > config api_base > site default
    let api_override = savhub_local::registry::read_api_base_url();
    let api_base = cli
        .api_base
        .clone()
        .or_else(|| std::env::var("SAVHUB_API_BASE").ok())
        .or(api_override)
        .unwrap_or_else(|| site.clone());
    Ok(GlobalOpts {
        workdir,
        dir,
        api_base,
        input_allowed: !cli.no_input,
    })
}

fn resolve_workdir(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = cli.workdir.clone() {
        return Ok(path.canonicalize().unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }));
    }
    if let Some(path) = std::env::var_os("SAVHUB_WORKDIR") {
        let path = PathBuf::from(path);
        return Ok(path.canonicalize().unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }));
    }
    std::env::current_dir().context("failed to resolve current directory")
}

async fn cmd_search(opts: &GlobalOpts, args: SearchArgs) -> Result<()> {
    let query = args.query.join(" ").trim().to_string();
    if query.is_empty() {
        bail!("query required");
    }
    let client = optional_client(opts)?;
    let mut url = client.v1_url("/search")?;
    url.query_pairs_mut()
        .append_pair("q", &query)
        .append_pair("kind", "skill");
    if let Some(limit) = args.limit {
        url.query_pairs_mut()
            .append_pair("limit", &limit.to_string());
    }
    let response = client.get_json_url::<SearchResponse>(url).await?;
    if response.results.is_empty() {
        println!("No results.");
        return Ok(());
    }
    for entry in response.results {
        let version = entry
            .latest_version
            .as_deref()
            .map(|value| format!(" v{value}"))
            .unwrap_or_default();
        let owner = entry
            .owner_handle
            .as_deref()
            .map(|handle| format!("  @{handle}"))
            .unwrap_or_default();
        println!(
            "{}{}  {}{}  ({:.3})",
            entry.slug, version, entry.display_name, owner, entry.score
        );
    }
    Ok(())
}

fn cmd_enable(opts: &GlobalOpts, args: EnableArgs) -> Result<()> {
    let result = enable_repo_skill_in_project(&opts.workdir, &args.repo, &args.slug)?;

    let revision = result
        .version
        .as_deref()
        .map(|v| format!("v{v}"))
        .or(result
            .git_sha
            .as_deref()
            .map(|v| v.chars().take(12).collect::<String>()))
        .unwrap_or_else(|| "latest".to_string());

    if result.local_name != result.slug {
        println!(
            "Enabled {} from repo '{}' as '{}' (renamed to avoid conflict) ({revision}).",
            result.slug, args.repo, result.local_name
        );
    } else {
        println!(
            "Enabled {} from repo '{}' ({revision}).",
            result.slug, args.repo
        );
    }

    Ok(())
}

fn cmd_disable(opts: &GlobalOpts, args: DisableArgs) -> Result<()> {
    if !args.yes {
        ensure_confirmed(
            opts.input_allowed,
            &format!("Disable {}?", args.slug),
            "pass --yes when input is disabled",
        )?;
    }

    let slug = normalize_slug(&args.slug)?;
    if disable_project_skill(&opts.workdir, &slug)? {
        println!("Disabled {slug}");
        Ok(())
    } else {
        bail!("{slug} is not enabled in this project")
    }
}

async fn cmd_fetch(opts: &GlobalOpts, args: FetchArgs) -> Result<()> {
    let slug = normalize_slug(&args.slug)?;
    let target = opts.dir.join(&slug);
    if target.exists() {
        if !args.force {
            bail!(
                "{} already exists; use --force to overwrite",
                target.display()
            );
        }
        fs::remove_dir_all(&target)
            .with_context(|| format!("failed to remove {}", target.display()))?;
    }

    let client = optional_client(opts)?;
    let resolved = resolve_remote_skill_fetch(&client, &slug).await?;
    install_remote_skill_from_repo(&resolved.spec, &target)?;

    let now = now_millis();
    write_repo_skill_origin(
        &target,
        &RepoSkillOrigin {
            version: 1,
            repo: opts.api_base.clone(),
            repo_sign: resolved.spec.repo_sign.clone(),
            repo_commit: Some(resolved.spec.git_sha.clone()),
            slug: slug.clone(),
            skill_version: resolved.spec.skill_version.clone(),
            fetched_at: now,
        },
    )?;
    let mut lockfile = read_project_added_skills(&opts.workdir)?;
    lockfile.insert(
        &resolved.spec.repo_sign,
        &resolved.spec.git_sha,
        &resolved.spec.skill_path,
        LockSkill {
            path: resolved.spec.skill_path.clone(),
            slug: slug.clone(),
            version: resolved.display_version.clone(),
        },
    );
    write_project_added_skills(&opts.workdir, &lockfile)?;
    println!(
        "Fetched {slug}@{} -> {}",
        resolved.display_version,
        target.display()
    );
    Ok(())
}

fn cmd_update(opts: &GlobalOpts, _args: UpdateArgs) -> Result<()> {
    // Read the project lock (savhub.lock) to find current skills + git_sha
    let project_lock = savhub_local::project::read_project_lockfile(&opts.workdir)?;
    if project_lock.skills.is_empty() {
        println!("No project skills in savhub.lock.");
        return Ok(());
    }

    // Read the central fetched.json to get the latest git_sha per repo
    let config_dir = savhub_local::config::get_config_dir()?;
    let fetched = read_lockfile(&config_dir)?;

    // Build a map: repo_url -> latest git_sha from fetched.json
    let fetched_sha: std::collections::HashMap<&str, &str> = fetched
        .repos
        .iter()
        .map(|r| (r.git_url.as_str(), r.git_sha.as_str()))
        .collect();

    let mut updated = 0usize;
    let mut skipped = 0usize;
    let mut new_skills = Vec::new();

    for skill in &project_lock.skills {
        let slug = &skill.slug;
        let repo_url = match skill.repo.as_deref().filter(|s| !s.is_empty()) {
            Some(r) => r,
            None => {
                println!("  {slug}: no repo info, skipped");
                new_skills.push(skill.clone());
                skipped += 1;
                continue;
            }
        };
        let skill_path = match skill.path.as_deref().filter(|s| !s.is_empty()) {
            Some(p) => p,
            None => {
                println!("  {slug}: no path info, skipped");
                new_skills.push(skill.clone());
                skipped += 1;
                continue;
            }
        };

        let latest_sha = fetched_sha.get(repo_url).copied().unwrap_or("");
        if latest_sha.is_empty() {
            println!("  {slug}: not in fetched.json, skipped");
            new_skills.push(skill.clone());
            skipped += 1;
            continue;
        }

        let local_sha = skill.git_sha.as_deref().unwrap_or("");
        if local_sha == latest_sha {
            new_skills.push(skill.clone());
            skipped += 1;
            continue;
        }

        // Copy from repo cache into project skills dir
        let source = savhub_local::registry::repo_skill_local_path(repo_url, skill_path);
        let Some(source) = source.filter(|p| p.is_dir()) else {
            println!("  {slug}: repo cache not found, run `savhub fetched --update` first");
            new_skills.push(skill.clone());
            skipped += 1;
            continue;
        };

        let target = opts.dir.join(slug);
        if target.exists() {
            fs::remove_dir_all(&target)
                .with_context(|| format!("failed to remove {}", target.display()))?;
        }
        savhub_local::skills::copy_skill_folder(&source, &target)?;

        let short_old = &local_sha[..12.min(local_sha.len())];
        let short_new = &latest_sha[..12.min(latest_sha.len())];
        println!("  {slug}: {short_old} -> {short_new}");

        new_skills.push(savhub_local::project::ProjectLockedSkill {
            repo: skill.repo.clone(),
            path: skill.path.clone(),
            slug: slug.clone(),
            version: skill.version.clone(),
            git_sha: Some(latest_sha.to_string()),
        });
        updated += 1;
    }

    savhub_local::project::write_project_lockfile(
        &opts.workdir,
        &savhub_local::project::ProjectLockFile {
            version: 1,
            skills: new_skills,
        },
    )?;

    println!("\nDone. {updated} updated, {skipped} already up-to-date.");
    Ok(())
}

/// Prune fetched.json: remove repos/flocks/skills not used by any project.
fn cmd_fetched_prune(config_dir: &Path, lockfile: &savhub_shared::Lockfile) -> Result<()> {
    use std::collections::HashSet;

    // Collect all (repo_url, skill_path) pairs used across all projects
    let projects = savhub_local::config::read_projects_list().unwrap_or_default();
    let mut used: HashSet<(String, String)> = HashSet::new();

    for project in &projects.projects {
        let project_path = PathBuf::from(&project.path);
        let project_lock =
            savhub_local::project::read_project_lockfile(&project_path).unwrap_or_default();
        for skill in &project_lock.skills {
            if let (Some(repo), Some(path)) = (skill.repo.as_deref(), skill.path.as_deref())
                && !repo.is_empty()
                && !path.is_empty()
            {
                used.insert((repo.to_string(), path.to_string()));
            }
        }
    }

    println!(
        "Scanning {} project(s), {} skill ref(s) in use.",
        projects.projects.len(),
        used.len()
    );

    // Walk the lockfile and keep only used entries
    let mut new_lockfile = savhub_shared::Lockfile::default();
    let mut kept_skills = 0usize;
    let mut removed_skills = 0usize;

    for repo in &lockfile.repos {
        let mut new_flocks = Vec::new();
        for flock in &repo.flocks {
            let mut new_skills = Vec::new();
            for skill in &flock.skills {
                if used.contains(&(repo.git_url.clone(), skill.path.clone())) {
                    new_skills.push(skill.clone());
                    kept_skills += 1;
                } else {
                    println!(
                        "  removing: {} ({}:{})",
                        skill.slug, repo.git_url, skill.path
                    );
                    removed_skills += 1;
                }
            }
            if !new_skills.is_empty() {
                new_flocks.push(savhub_shared::LockFlock {
                    path: flock.path.clone(),
                    skills: new_skills,
                });
            }
        }
        if !new_flocks.is_empty() {
            new_lockfile.repos.push(savhub_shared::LockRepo {
                git_url: repo.git_url.clone(),
                git_sha: repo.git_sha.clone(),
                flocks: new_flocks,
            });
        } else if !repo.flocks.is_empty() {
            println!("  removing repo: {} (no skills in use)", repo.git_url);
        }
    }

    savhub_local::skills::write_lockfile(config_dir, &new_lockfile)?;
    println!(
        "\nDone. {kept_skills} kept, {removed_skills} removed, {} repo(s) remaining.",
        new_lockfile.repos.len()
    );
    Ok(())
}

async fn cmd_fetched(opts: &GlobalOpts, args: FetchedArgs) -> Result<()> {
    let config_dir = savhub_local::config::get_config_dir()?;
    let mut lockfile = read_lockfile(&config_dir)?;

    if args.prune {
        return cmd_fetched_prune(&config_dir, &lockfile);
    }

    if !args.update {
        if lockfile.is_empty() {
            println!("No fetched skills.");
            return Ok(());
        }
        for repo in &lockfile.repos {
            println!("{}  {}", repo.git_url, repo.git_sha);
            for flock in &repo.flocks {
                for skill in &flock.skills {
                    println!("  {}  {}", skill.slug, skill.version);
                }
            }
        }
        return Ok(());
    }

    if lockfile.repos.is_empty() {
        println!("No fetched skills. Use `savhub fetch <slug>` first.");
        return Ok(());
    }

    println!("Updating {} repo(s)...", lockfile.repos.len());
    let client = optional_client(opts)?;
    let mut repos_updated = 0usize;
    let mut repos_skipped = 0usize;
    let mut repos_failed = 0usize;
    let skill_count: usize = lockfile
        .repos
        .iter()
        .flat_map(|r| &r.flocks)
        .map(|f| f.skills.len())
        .sum();

    for repo in &mut lockfile.repos {
        let repo_path = git_url_to_route_path(&repo.git_url);
        let detail = match client
            .get_json::<RepoDetailResponse>(&format!("/repos/{repo_path}"))
            .await
        {
            Ok(d) => d,
            Err(err) => {
                println!("  {}: failed - {err}", repo.git_url);
                repos_failed += 1;
                continue;
            }
        };

        let remote_sha = match normalize_remote_text(detail.document.git_sha.clone()) {
            Some(sha) => sha,
            None => {
                println!("  {}: no git_sha from registry", repo.git_url);
                repos_failed += 1;
                continue;
            }
        };

        if repo.git_sha == remote_sha && !args.force {
            let skill_names: Vec<_> = repo
                .flocks
                .iter()
                .flat_map(|f| &f.skills)
                .map(|s| s.slug.as_str())
                .collect();
            println!(
                "  {}: already at {} ({} skills: {})",
                repo.git_url,
                &remote_sha[..12.min(remote_sha.len())],
                skill_names.len(),
                skill_names.join(", ")
            );
            repos_skipped += 1;
            continue;
        }

        // Update repo checkout
        match savhub_local::registry::ensure_repo_checkout(
            &repo.git_url,
            &detail.document.git_url,
            &remote_sha,
        ) {
            Ok(_) => {
                let old_sha = &repo.git_sha;
                let short_old = &old_sha[..12.min(old_sha.len())];
                let short_new = &remote_sha[..12.min(remote_sha.len())];
                let skill_names: Vec<_> = repo
                    .flocks
                    .iter()
                    .flat_map(|f| &f.skills)
                    .map(|s| s.slug.as_str())
                    .collect();
                println!(
                    "  {}: {} -> {} ({} skills: {})",
                    repo.git_url,
                    short_old,
                    short_new,
                    skill_names.len(),
                    skill_names.join(", ")
                );
                repo.git_sha = remote_sha;
                repos_updated += 1;
            }
            Err(err) => {
                println!("  {}: failed - {err}", repo.git_url);
                repos_failed += 1;
            }
        }
    }

    savhub_local::skills::write_lockfile(&config_dir, &lockfile)?;
    println!(
        "\nDone. {repos_updated} repo(s) updated, {repos_skipped} already up-to-date, {repos_failed} failed. ({skill_count} skills total)"
    );
    Ok(())
}

fn cmd_prune(opts: &GlobalOpts, args: PruneArgs) -> Result<()> {
    let slug = normalize_slug(&args.slug)?;
    if !args.yes {
        ensure_confirmed(
            opts.input_allowed,
            &format!("Prune {slug}?"),
            "pass --yes when input is disabled",
        )?;
    }
    if !disable_project_skill(&opts.workdir, &slug)? {
        bail!("{slug} is not fetched");
    }
    println!("Pruned {slug}");
    Ok(())
}

fn cmd_list(opts: &GlobalOpts) -> Result<()> {
    let lockfile = read_project_added_skills(&opts.workdir)?;
    if lockfile.is_empty() {
        println!("No fetched skills.");
        return Ok(());
    }
    for (_, _, _, skill) in lockfile.iter_skills() {
        println!("{}  {}", skill.slug, skill.version);
    }
    Ok(())
}

async fn cmd_explore(opts: &GlobalOpts, args: ExploreArgs) -> Result<()> {
    let client = optional_client(opts)?;
    let mut url = client.v1_url("/skills")?;
    let sort = map_explore_sort(&args.sort);
    url.query_pairs_mut()
        .append_pair("limit", &args.limit.clamp(1, 100).to_string());
    if sort != "updated" {
        url.query_pairs_mut().append_pair("sort", sort);
    }
    let response = client
        .get_json_url::<PagedResponse<SkillListItem>>(url)
        .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    if response.items.is_empty() {
        println!("No skills found.");
        return Ok(());
    }
    for item in response.items {
        let version = item
            .latest_version
            .as_ref()
            .map(|value| value.version.as_str())
            .unwrap_or("?");
        let age = relative_time(item.updated_at);
        let summary = item
            .summary
            .as_deref()
            .map(|summary| format!("  {}", truncate(summary, 64)))
            .unwrap_or_default();
        println!("{}  v{}  {}{}", item.slug, version, age, summary);
    }
    Ok(())
}


// Dormant admin commands (publish / moderate / sync / ban / role flows) moved
// to `admin.rs`. They are kept compiling but not wired into the `Command`
// enum; see that module's header for the rationale and how to re-enable them.

#[derive(Debug, Clone)]
struct ResolvedRemoteSkillFetch {
    spec: RemoteSkillFetchSpec,
    display_version: String,
}

fn normalize_remote_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Convert a git URL to the route path used by the API (strip scheme and .git suffix).
fn git_url_to_route_path(url: &str) -> String {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .to_string()
}

fn repo_sign_from_skill_detail(detail: &SkillDetailResponse) -> Result<String> {
    let repo_url = detail.skill.repo_url.trim();
    if repo_url.is_empty() {
        bail!("skill `{}` is missing repo_url metadata", detail.skill.slug);
    }
    Ok(repo_url.to_string())
}

async fn resolve_remote_skill_summary(client: &ApiClient, slug: &str) -> Result<SkillListItem> {
    let mut url = client.v1_url("/skills")?;
    url.query_pairs_mut()
        .append_pair("limit", "50")
        .append_pair("q", slug);
    let response = client
        .get_json_url::<PagedResponse<SkillListItem>>(url)
        .await?;
    response
        .items
        .into_iter()
        .find(|item| item.slug.eq_ignore_ascii_case(slug))
        .ok_or_else(|| anyhow!("skill `{slug}` does not exist"))
}

async fn resolve_remote_skill_fetch(
    client: &ApiClient,
    slug: &str,
) -> Result<ResolvedRemoteSkillFetch> {
    let summary = resolve_remote_skill_summary(client, slug).await?;
    let detail = client
        .get_json::<SkillDetailResponse>(&format!("/skills/{}", summary.id))
        .await?;
    let repo_url = repo_sign_from_skill_detail(&detail)?;
    let repo_path = git_url_to_route_path(&repo_url);
    let repo = client
        .get_json::<RepoDetailResponse>(&format!("/repos/{repo_path}"))
        .await?;
    let git_sha = normalize_remote_text(repo.document.git_sha.clone())
        .ok_or_else(|| anyhow!("repo `{repo_url}` has no git_sha"))?;
    let skill_version = normalize_remote_text(
        detail
            .latest_version
            .as_ref()
            .map(|value| value.version.clone())
            .or_else(|| detail.versions.first().map(|value| value.version.clone())),
    );
    let display_version = fetch_version_label(skill_version.as_deref(), &git_sha);
    Ok(ResolvedRemoteSkillFetch {
        spec: RemoteSkillFetchSpec {
            repo_sign: repo_url,
            skill_path: detail.skill.path.clone(),
            git_url: repo.document.git_url,
            git_sha,
            skill_version,
        },
        display_version,
    })
}


pub(crate) fn authed_client(opts: &GlobalOpts) -> Result<ApiClient> {
    Ok(ApiClient::new(&opts.api_base, Some(require_auth_token()?)))
}

pub(crate) fn optional_client(opts: &GlobalOpts) -> Result<ApiClient> {
    Ok(ApiClient::new(
        &opts.api_base,
        read_global_config()?.and_then(|config| config.token),
    ))
}

fn require_auth_token() -> Result<String> {
    read_global_config()?
        .and_then(|config| config.token)
        .ok_or_else(|| anyhow!("not logged in; run `savhub login`"))
}

fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

pub(crate) fn normalize_slug(value: &str) -> Result<String> {
    let slug = value.trim().to_lowercase();
    if slug.is_empty() || slug.contains('/') || slug.contains('\\') || slug.contains("..") {
        bail!("invalid slug: {value}");
    }
    Ok(slug)
}

pub(crate) fn ensure_confirmed(input_allowed: bool, prompt: &str, disabled_message: &str) -> Result<()> {
    if !input_allowed {
        bail!(disabled_message.to_string());
    }
    let confirmed = Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()
        .map_err(|error| anyhow!("failed to read confirmation: {error}"))?;
    if confirmed { Ok(()) } else { bail!("canceled") }
}

fn map_explore_sort(value: &str) -> &'static str {
    match value.trim().to_lowercase().as_str() {
        "" | "newest" | "updated" | "trending" => "updated",
        "downloads" | "download" => "downloads",
        "installs" | "install" | "installsalltime" | "installs-all-time" => "installs",
        "users" | "used" | "most-used" => "users",
        "rating" | "stars" | "star" => "stars",
        "name" => "name",
        _ => "updated",
    }
}


fn relative_time(timestamp: DateTime<Utc>) -> String {
    let now = Utc::now();
    let delta = now - timestamp;
    if delta.num_days() > 30 {
        format!("{}mo ago", delta.num_days() / 30)
    } else if delta.num_days() > 0 {
        format!("{}d ago", delta.num_days())
    } else if delta.num_hours() > 0 {
        format!("{}h ago", delta.num_hours())
    } else if delta.num_minutes() > 0 {
        format!("{}m ago", delta.num_minutes())
    } else {
        "just now".to_string()
    }
}

pub(crate) fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        value
            .chars()
            .take(max.saturating_sub(3))
            .collect::<String>()
            + "..."
    }
}

// ---------------------------------------------------------------------------
// registry subcommands
// ---------------------------------------------------------------------------

async fn cmd_registry(_opts: &GlobalOpts, command: RegistryCommand) -> Result<()> {
    use savhub_local::registry;

    match command {
        RegistryCommand::Search(args) => {
            let query = args.query.join(" ");
            let results = registry::search_skills(&query, args.limit)?;
            if results.is_empty() {
                println!("No skills matching \"{query}\".");
                return Ok(());
            }
            for skill in &results {
                let summary = truncate(skill.description.as_deref().unwrap_or(""), 60);
                println!(
                    "  {:<30} v{:<10} {}",
                    skill.slug,
                    skill.version.as_deref().unwrap_or("-"),
                    summary
                );
            }
            println!("\n{} result(s)", results.len());
        }
        RegistryCommand::List(args) => {
            let page = args.page.saturating_sub(1); // user-facing 1-based
            let (skills, total) = registry::list_skills(
                args.query.as_deref(),
                args.status.as_deref(),
                page,
                args.page_size,
            )?;

            if args.json {
                let out = serde_json::json!({
                    "items": skills,
                    "total": total,
                    "page": args.page,
                    "page_size": args.page_size,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(());
            }

            if skills.is_empty() {
                println!("No skills found.");
                return Ok(());
            }

            for skill in &skills {
                let summary = truncate(skill.description.as_deref().unwrap_or(""), 55);
                println!(
                    "  {:<30} v{:<10} {}",
                    skill.slug,
                    skill.version.as_deref().unwrap_or("-"),
                    summary
                );
            }

            let total_pages = total.div_ceil(args.page_size);
            println!(
                "\nPage {}/{} ({} total skills)",
                args.page, total_pages, total
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// selector subcommands
// ---------------------------------------------------------------------------

fn cmd_selector(opts: &GlobalOpts, command: SelectorCommand) -> Result<()> {
    use savhub_local::selectors::{read_selectors_store, run_selectors};

    match command {
        SelectorCommand::List => {
            // Sync official selectors from the server
            if let Err(e) = savhub_local::selectors::sync_official_selectors(&opts.api_base) {
                eprintln!("Warning: could not sync official selectors: {e}");
            }
            let official_store = savhub_local::selectors::read_official_selectors_store()?;
            let custom_store = read_selectors_store()?;
            let prefs = savhub_local::selectors::read_selector_prefs().unwrap_or_default();

            let mut all_selectors: Vec<savhub_local::selectors::SelectorDefinition> = Vec::new();
            for entry in &official_store.selectors {
                all_selectors.push(entry.selector.clone());
            }
            all_selectors.extend(custom_store.selectors.clone());

            if all_selectors.is_empty() {
                println!("No selectors configured. Run `savhub apply` to sync official selectors.");
                return Ok(());
            }
            for d in &all_selectors {
                let pri = if d.priority != 0 {
                    format!(" [P{}]", d.priority)
                } else {
                    String::new()
                };
                let enabled = !prefs.disabled.contains(&d.sign);
                let status = if enabled { "+" } else { "-" };
                let official_tag = if savhub_local::selectors::is_official_selector(&d.sign) {
                    " (official)"
                } else {
                    ""
                };
                let rules = d.rules.len();
                let skills = d.skills.len();
                println!(
                    "  [{status}] {:<24} scope={:<10} {}r {}s{}{}",
                    d.name, d.folder_scope, rules, skills, pri, official_tag
                );
                if !d.description.is_empty() {
                    let desc: String = d.description.chars().take(80).collect();
                    println!("      {desc}");
                }
            }
            println!("\n{} selector(s)", all_selectors.len());
        }
        SelectorCommand::Show(args) => {
            let official_store = savhub_local::selectors::read_official_selectors_store()?;
            let custom_store = read_selectors_store()?;
            let mut all: Vec<savhub_local::selectors::SelectorDefinition> = Vec::new();
            for entry in &official_store.selectors {
                all.push(entry.selector.clone());
            }
            all.extend(custom_store.selectors.clone());
            let query = args.name.to_lowercase();
            let found: Vec<_> = all
                .iter()
                .filter(|d| {
                    d.name.to_lowercase().contains(&query) || d.sign.to_lowercase().contains(&query)
                })
                .collect();
            if found.is_empty() {
                println!(
                    "No selector matching \"{}\". Use `savhub selector list` to see all.",
                    args.name
                );
                return Ok(());
            }
            for d in &found {
                println!("Name:       {}", d.name);
                println!("ID:         {}", d.sign);
                let enabled = savhub_local::selectors::is_selector_enabled(&d.sign);
                println!("Enabled:    {}", if enabled { "yes" } else { "no" });
                if !d.description.is_empty() {
                    println!("Desc:       {}", d.description);
                }
                println!("Scope:      {}", d.folder_scope);
                println!("Mode:       {:?}", d.match_mode);
                if !d.custom_expression.is_empty() {
                    println!("Expression: {}", d.custom_expression);
                } else {
                    println!("Expression: {}", d.display_expression());
                }
                println!("Priority:   {}", d.priority);
                println!("Rules:");
                for (i, rule) in d.rules.iter().enumerate() {
                    println!("  {}. {}", i + 1, rule.display());
                }
                if !d.skills.is_empty() {
                    println!(
                        "Skills:     {}",
                        d.skills
                            .iter()
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                if !d.flocks.is_empty() {
                    let flock_strs: Vec<String> = d.flocks.iter().map(|s| s.to_string()).collect();
                    for (repo, members) in tui::group_flocks_by_repo(&flock_strs) {
                        println!("Flock {repo}");
                        for f in &members {
                            println!("  {}", tui::flock_display(f));
                        }
                    }
                }
                if !d.repos.is_empty() {
                    let repo_strs: Vec<_> = d.repos.iter().map(|r| r.git_url.as_str()).collect();
                    println!("Repos:      {}", repo_strs.join(", "));
                }
                println!();
            }
        }
        SelectorCommand::Test => {
            let result = run_selectors(&opts.workdir)?;
            if result.matched.is_empty() {
                println!("No selectors matched {}.", opts.workdir.display());
                return Ok(());
            }
            println!("Matched selectors for {}:", opts.workdir.display());
            for m in &result.matched {
                let pri = m.selector.priority;
                println!(
                    "  [+] {} (P{pri}) — {}",
                    m.selector.name, m.selector.description
                );
            }
            if !result.skills.is_empty() {
                println!(
                    "Skills:  {}",
                    result
                        .skills
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if !result.flocks.is_empty() {
                let flock_strs: Vec<String> = result.flocks.iter().map(|s| s.to_string()).collect();
                for (repo, members) in tui::group_flocks_by_repo(&flock_strs) {
                    println!("Flock {repo}");
                    for f in &members {
                        println!("  {}", tui::flock_display(f));
                    }
                }
            }
            if !result.repos.is_empty() {
                let repo_strs: Vec<_> = result.repos.iter().map(|r| r.git_url.as_str()).collect();
                println!("Repos:   {}", repo_strs.join(", "));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// flock commands
// ---------------------------------------------------------------------------

fn cmd_flock(_opts: &GlobalOpts, command: FlockCommand) -> Result<()> {
    use savhub_local::registry;

    match command {
        FlockCommand::List => {
            let flocks = registry::list_flocks()?;
            if flocks.is_empty() {
                println!("No flocks found.");
                return Ok(());
            }
            for flock in &flocks {
                let skill_count = registry::list_flock_skills(&flock.repo, &flock.slug)
                    .map(|s| s.len())
                    .unwrap_or(0);
                println!(
                    "  {:<24} {:>3} skill(s)  {}",
                    flock.slug, skill_count, flock.name
                );
                if !flock.description.is_empty() {
                    let desc: String = flock.description.chars().take(72).collect();
                    println!("    {desc}");
                }
            }
            println!("\n{} flock(s)", flocks.len());
        }
        FlockCommand::Show(args) => {
            let flock_ref = savhub_local::selectors::SelectorSkillRef::parse(&args.slug);
            let flock = registry::get_flock_by_slug(&flock_ref.repo, &flock_ref.path)?;
            let Some(flock) = flock else {
                println!(
                    "Flock \"{}\" not found. Run `savhub flock list` to see available flocks.",
                    args.slug
                );
                return Ok(());
            };
            println!("Name:        {}", flock.name);
            println!("Slug:        {}", flock.slug);
            if !flock.description.is_empty() {
                println!("Description: {}", flock.description);
            }
            let skills = registry::list_skills_in_flock(&flock.repo, &flock.slug)?;
            if skills.is_empty() {
                println!("Skills:      (none)");
            } else {
                println!("Skills ({}):", skills.len());
                for skill in &skills {
                    println!("  - {}  {}", skill.slug, skill.name);
                }
            }
        }
        FlockCommand::Fetch(args) => {
            let flock_ref = savhub_local::selectors::SelectorSkillRef::parse(&args.slug);
            let flock = registry::get_flock_by_slug(&flock_ref.repo, &flock_ref.path)?;
            let Some(flock) = flock else {
                println!("Flock \"{}\" not found.", args.slug);
                return Ok(());
            };
            let skill_slugs = registry::list_flock_skills(&flock.repo, &flock.slug)?;
            if skill_slugs.is_empty() {
                println!("Flock \"{}\" has no skills.", flock.slug);
                return Ok(());
            }
            println!("Flock: {} ({})", flock.name, flock.slug);
            println!("Skills to fetch:");
            for slug in &skill_slugs {
                println!("  [+] {slug}");
            }
            if !args.yes {
                let proceed = Confirm::new()
                    .with_prompt(format!(
                        "Fetch {} skill(s) from flock \"{}\"?",
                        skill_slugs.len(),
                        flock.slug
                    ))
                    .default(true)
                    .interact()?;
                if !proceed {
                    println!("Cancelled.");
                    return Ok(());
                }
            }
            let mut lockfile = read_project_added_skills(&_opts.workdir)?;
            let mut added = 0;
            for slug in &skill_slugs {
                if lockfile.find_by_slug(slug).is_none() {
                    lockfile.insert(
                        "unknown",
                        "",
                        slug,
                        LockSkill {
                            path: slug.clone(),
                            slug: slug.clone(),
                            version: "latest".to_string(),
                        },
                    );
                    added += 1;
                    println!("  Added: {slug}");
                } else {
                    println!("  Already installed: {slug}");
                }
            }
            if added > 0 {
                write_project_added_skills(&_opts.workdir, &lockfile)?;
            }
            println!(
                "\nDone. {added} skill(s) added from flock \"{}\".",
                flock.slug
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// pilot command
// ---------------------------------------------------------------------------

fn cmd_pilot(command: PilotCommand) -> Result<()> {
    use savhub_local::pilot;

    // Resolve agents: use --agents flag, or fall back to config
    let resolve_agents = |args: &PilotAgentArgs| -> Result<Vec<String>> {
        if !args.agents.is_empty() {
            return Ok(args.agents.clone());
        }
        let cfg = savhub_local::config::read_global_config()?.unwrap_or_default();
        if cfg.agents.is_empty() {
            // Default to claude-code if nothing configured
            Ok(vec!["claude-code".to_string()])
        } else {
            Ok(cfg.agents)
        }
    };

    match command {
        PilotCommand::Install(args) => {
            let agents = resolve_agents(&args)?;
            let (shared, agent_dirs) = pilot::install(&agents)?;
            println!("Installed savhub skills (savhub-selector-editor, savhub-skill-manager):\n");
            println!("  shared:");
            println!("    {}", shared.join("SKILL.md").display());
            for (agent, dir) in &agent_dirs {
                println!("  {agent}:");
                println!("    {}", dir.join("SKILL.md").display());
            }
            println!("\nRun `savhub apply` in your project to activate.");
        }
        PilotCommand::Uninstall(args) => {
            let agents = resolve_agents(&args)?;
            let removed = pilot::uninstall(&agents)?;
            if removed.is_empty() {
                println!("Nothing to remove.");
            } else {
                for dir in &removed {
                    println!("  Removed: {}", dir.display());
                }
            }
        }
        PilotCommand::Status(args) => {
            let agents = resolve_agents(&args)?;
            let statuses = pilot::status(&agents)?;
            for (agent, path) in &statuses {
                if let Some(p) = path {
                    println!("  {agent}: installed at {}", p.display());
                } else {
                    println!("  {agent}: not installed");
                }
            }
        }
        PilotCommand::Notify => {
            pilot::notify_config_changed()?;
            println!("Config change signal sent.");
        }
    }
    Ok(())
}
