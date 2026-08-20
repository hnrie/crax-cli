//! `grok skill` CLI subcommand — browse and install skills from the marketplace.
//!
//! Follows the `plugin_cmd.rs` / `mcp_cmd.rs` pattern: clap args and handler
//! logic co-located in a dedicated module, with `main.rs` dispatching here in
//! one line. Business logic lives in `xai_grok_plugin_marketplace`; this module
//! parses arguments, formats output, and picks the install scope.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::Serialize;

use xai_grok_plugin_marketplace::skill_install::{
    InstallAction, InstallOptions, InstalledSkill, SkillInstallError,
};
use xai_grok_plugin_marketplace::skill_lock::SkillLock;
use xai_grok_plugin_marketplace::skill_registry::{RegistryClient, RegistryError, RegistrySkill};
use xai_grok_plugin_marketplace::skill_scope::{SkillScopeKind, SkillScopeTarget};
use xai_grok_plugin_marketplace::skill_source::{SkillSource, parse_skill_source};

/// Default number of search results shown without `--limit`.
const DEFAULT_SEARCH_LIMIT: usize = 20;

// ── CLI arg definitions ─────────────────────────────────────────────

#[derive(Debug, clap::Args, Clone)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: SkillCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum SkillCommand {
    /// Search the skill marketplace
    #[command(visible_alias = "find")]
    Search {
        /// Search terms. Multi-word queries use semantic matching.
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Maximum number of results (1-200).
        #[arg(long, default_value_t = DEFAULT_SEARCH_LIMIT)]
        limit: usize,
        /// Restrict results to one GitHub owner.
        #[arg(long)]
        owner: Option<String>,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Show a marketplace skill's details without installing it
    #[command(visible_alias = "show")]
    Info {
        /// Registry id (`owner/repo/skill`), repository, or local path.
        source: String,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Install a skill from the marketplace, a repository, or a local path
    #[command(visible_alias = "add")]
    Install {
        /// Registry id (`owner/repo/skill`), GitHub shorthand (`owner/repo`),
        /// a git URL, or a local path. Supports `@skill` and `#ref` suffixes.
        source: String,
        /// Install for this project instead of the current user.
        #[arg(long, value_name = "SCOPE", default_value = "user")]
        scope: String,
        /// Install only this skill from a source that provides several.
        #[arg(long)]
        only: Option<String>,
        /// Overwrite an existing skill, including one with local edits.
        #[arg(long)]
        force: bool,
        /// Show what would be installed without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// List installed skills
    #[command(visible_alias = "ls")]
    List {
        /// Limit the listing to one scope (`user` or `project`).
        #[arg(long, value_name = "SCOPE")]
        scope: Option<String>,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Update installed skill(s) from their recorded sources
    Update {
        /// Skill name to update. Omit to update every tracked skill.
        name: Option<String>,
        /// Limit the update to one scope (`user` or `project`).
        #[arg(long, value_name = "SCOPE")]
        scope: Option<String>,
        /// Replace skills that have local edits.
        #[arg(long)]
        force: bool,
        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Remove an installed skill
    #[command(visible_alias = "rm", visible_alias = "uninstall")]
    Remove {
        /// Skill name (as shown by `grok skill list`).
        name: String,
        /// Limit the removal to one scope (`user` or `project`).
        #[arg(long, value_name = "SCOPE")]
        scope: Option<String>,
        /// Drop the install record but leave the files in place.
        #[arg(long)]
        keep_files: bool,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
}

// ── JSON output types ───────────────────────────────────────────────

#[derive(Serialize)]
struct SearchResultJson {
    id: String,
    name: String,
    source: String,
    installs: u64,
    url: String,
    install: String,
}

#[derive(Serialize)]
struct SkillInfoJson {
    source: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    registry_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subpath: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill_md_preview: Option<String>,
}

#[derive(Serialize)]
struct InstalledSkillJson {
    name: String,
    description: String,
    path: PathBuf,
    action: String,
    files: usize,
    scope: String,
}

#[derive(Serialize)]
struct ListedSkillJson {
    name: String,
    scope: String,
    path: PathBuf,
    tracked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    installed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

#[derive(Serialize)]
struct RemovedSkillJson {
    name: String,
    scope: String,
    path: PathBuf,
    files_removed: bool,
}

// ── Top-level dispatch ──────────────────────────────────────────────

pub async fn run(args: SkillArgs) -> Result<()> {
    match args.command {
        SkillCommand::Search {
            query,
            limit,
            owner,
            json,
        } => cmd_search(&query.join(" "), limit, owner.as_deref(), json).await,
        SkillCommand::Info { source, json } => cmd_info(&source, json).await,
        SkillCommand::Install {
            source,
            scope,
            only,
            force,
            dry_run,
            json,
        } => cmd_install(&source, &scope, only, force, dry_run, json).await,
        SkillCommand::List { scope, json } => cmd_list(scope.as_deref(), json),
        SkillCommand::Update {
            name,
            scope,
            force,
            dry_run,
            json,
        } => cmd_update(name.as_deref(), scope.as_deref(), force, dry_run, json).await,
        SkillCommand::Remove {
            name,
            scope,
            keep_files,
            json,
        } => cmd_remove(&name, scope.as_deref(), keep_files, json),
    }
}

// ── Scope resolution ────────────────────────────────────────────────

/// Resolve a `--scope` value into a concrete install target.
fn resolve_scope(scope: &str) -> Result<SkillScopeTarget> {
    let kind = SkillScopeKind::parse(scope)
        .with_context(|| format!("unknown scope `{scope}` (expected `user` or `project`)"))?;
    Ok(target_for(kind))
}

/// Build the install target for a scope kind.
fn target_for(kind: SkillScopeKind) -> SkillScopeTarget {
    match kind {
        SkillScopeKind::User => SkillScopeTarget::user(&xai_grok_config::grok_home()),
        SkillScopeKind::Project => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            SkillScopeTarget::project(&cwd)
        }
    }
}

/// Scopes to inspect for a read or multi-scope command.
///
/// Without an explicit `--scope`, both are listed so a skill is never silently
/// missing from output just because it lives in the other one.
fn scopes_to_inspect(scope: Option<&str>) -> Result<Vec<SkillScopeTarget>> {
    match scope {
        Some(value) => Ok(vec![resolve_scope(value)?]),
        None => Ok(vec![
            target_for(SkillScopeKind::User),
            target_for(SkillScopeKind::Project),
        ]),
    }
}

// ── Subcommands ─────────────────────────────────────────────────────

async fn cmd_search(query: &str, limit: usize, owner: Option<&str>, json: bool) -> Result<()> {
    let client = RegistryClient::new();
    let results = client
        .search(query, limit, owner)
        .await
        .map_err(describe_registry_error)?;

    if json {
        let payload: Vec<SearchResultJson> = results
            .iter()
            .map(|skill| SearchResultJson {
                id: skill.id.clone(),
                name: skill.name.clone(),
                source: skill.source.clone(),
                installs: skill.installs,
                url: skill.page_url(),
                install: skill.install_spec(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    if results.is_empty() {
        println!("No skills matched `{query}`.");
        return Ok(());
    }

    let name_width = results
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(40);
    for skill in &results {
        println!(
            "  {:<name_width$}  {:>9}  {}",
            truncate(&skill.name, name_width),
            format_installs(skill.installs),
            skill.source,
        );
    }
    println!(
        "\n{} result(s). Install one with: grok skill install {}",
        results.len(),
        example_spec(&results)
    );
    Ok(())
}

async fn cmd_info(source: &str, json: bool) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let parsed = parse_skill_source(source, &cwd, true)?;

    let mut payload = SkillInfoJson {
        source: source.to_string(),
        kind: parsed.kind().to_string(),
        registry_id: None,
        git_url: None,
        git_ref: None,
        subpath: None,
        hash: None,
        files: None,
        skill_md_preview: None,
    };

    match &parsed {
        SkillSource::Registry {
            source: registry_source,
            slug,
        } => {
            payload.registry_id = Some(format!("{registry_source}/{slug}"));
            let client = RegistryClient::new();
            let files = client
                .download_by_id(&format!("{registry_source}/{slug}"))
                .await
                .map_err(describe_registry_error)?;
            payload.hash = files.hash.clone();
            payload.files = Some(files.files.iter().map(|f| f.path.clone()).collect());
            payload.skill_md_preview = files.skill_md().map(|f| preview(&f.contents));
        }
        SkillSource::Git {
            url,
            git_ref,
            subpath,
            ..
        } => {
            payload.git_url = Some(url.clone());
            payload.git_ref.clone_from(git_ref);
            payload.subpath.clone_from(subpath);
        }
        SkillSource::Local { path } => {
            payload.git_url = Some(path.display().to_string());
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("  source: {source}");
    println!("  kind:   {}", payload.kind);
    if let Some(id) = &payload.registry_id {
        println!("  id:     {id}");
    }
    if let Some(url) = &payload.git_url {
        println!("  from:   {url}");
    }
    if let Some(git_ref) = &payload.git_ref {
        println!("  ref:    {git_ref}");
    }
    if let Some(subpath) = &payload.subpath {
        println!("  path:   {subpath}");
    }
    if let Some(hash) = &payload.hash {
        println!("  hash:   {}", &hash[..hash.len().min(12)]);
    }
    if let Some(files) = &payload.files {
        println!("  files:  {}", files.len());
        for path in files.iter().take(10) {
            println!("    {path}");
        }
        if files.len() > 10 {
            println!("    … {} more", files.len() - 10);
        }
    }
    if let Some(preview) = &payload.skill_md_preview {
        println!("\n{preview}");
    }
    Ok(())
}

async fn cmd_install(
    source: &str,
    scope: &str,
    only: Option<String>,
    force: bool,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let parsed = parse_skill_source(source, &cwd, true)?;
    let target = resolve_scope(scope)?;
    let options = InstallOptions {
        force,
        dry_run,
        only,
    };

    let installed = xai_grok_plugin_marketplace::skill_install::install(
        &parsed,
        source,
        &target,
        &options,
        &RegistryClient::new(),
    )
    .await
    .map_err(describe_install_error)?;

    report_installed(&installed, &target, dry_run, json)
}

fn cmd_list(scope: Option<&str>, json: bool) -> Result<()> {
    let targets = scopes_to_inspect(scope)?;
    let mut listed: Vec<ListedSkillJson> = Vec::new();

    for target in &targets {
        let lock = SkillLock::load(target.root());
        for name in target.installed_names() {
            let entry = lock.get(&name);
            listed.push(ListedSkillJson {
                path: target.skills_dir().join(&name),
                name,
                scope: target.kind().as_str().to_string(),
                tracked: entry.is_some(),
                source: entry.map(|e| e.source.clone()),
                origin: entry.map(|e| e.origin.as_str().to_string()),
                installed_at: entry.map(|e| e.installed_at.clone()),
                updated_at: entry.map(|e| e.updated_at.clone()),
            });
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&listed)?);
        return Ok(());
    }

    if listed.is_empty() {
        println!("No skills installed. Find some with `grok skill search <query>`.");
        return Ok(());
    }

    let name_width = listed
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(0);
    for skill in &listed {
        let provenance = match (&skill.origin, &skill.source) {
            (Some(origin), Some(source)) => format!("{origin}: {source}"),
            _ => "not installed from the marketplace".to_string(),
        };
        println!(
            "  {:<name_width$}  [{}]  {provenance}",
            skill.name, skill.scope
        );
    }
    Ok(())
}

async fn cmd_update(
    name: Option<&str>,
    scope: Option<&str>,
    force: bool,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let targets = scopes_to_inspect(scope)?;
    let registry = RegistryClient::new();
    let options = InstallOptions {
        force,
        dry_run,
        only: None,
    };

    let mut updated: Vec<(InstalledSkill, SkillScopeKind)> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut considered = 0usize;

    // Lockfile keys are normalized slugs, so a user typing `Deploy To Prod`
    // has to be normalized the same way before the lookup.
    let wanted = name.map(xai_grok_plugin_marketplace::skill_validate::normalize_name);

    for target in &targets {
        let lock = SkillLock::load(target.root());
        let names: Vec<String> = match &wanted {
            Some(one) => {
                if lock.contains(one) {
                    vec![one.clone()]
                } else {
                    vec![]
                }
            }
            None => lock.skills.keys().cloned().collect(),
        };

        for skill_name in names {
            considered += 1;
            match xai_grok_plugin_marketplace::skill_install::update(
                &skill_name,
                target,
                &options,
                &registry,
                &cwd,
            )
            .await
            {
                Ok(result) => updated.push((result, target.kind())),
                Err(error) => {
                    failures.push(format!("{skill_name}: {}", describe_install_error(error)));
                }
            }
        }
    }

    if considered == 0 {
        match name {
            Some(one) => bail!(
                "`{one}` is not tracked as a marketplace install; run `grok skill list` to see what is"
            ),
            None => {
                println!("No marketplace skills to update.");
                return Ok(());
            }
        }
    }

    if json {
        let payload: Vec<InstalledSkillJson> = updated
            .iter()
            .map(|(skill, kind)| installed_json(skill, *kind))
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        for (skill, kind) in &updated {
            println!(
                "  {} [{}] {}",
                skill.name,
                kind.as_str(),
                skill.action.as_str()
            );
        }
        // Count against everything attempted, so a run where every skill
        // failed does not report a misleading "0 of 0".
        if !updated.is_empty() {
            let changed = updated
                .iter()
                .filter(|(s, _)| s.action != InstallAction::Unchanged)
                .count();
            let verb = if dry_run { "would change" } else { "changed" };
            println!("\n{changed} of {considered} skill(s) {verb}.");
        }
    }

    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("  failed: {failure}");
        }
        bail!("{} skill(s) failed to update", failures.len());
    }
    Ok(())
}

fn cmd_remove(name: &str, scope: Option<&str>, keep_files: bool, json: bool) -> Result<()> {
    let targets = scopes_to_inspect(scope)?;
    let mut removed: Vec<RemovedSkillJson> = Vec::new();
    let mut last_error: Option<SkillInstallError> = None;

    for target in &targets {
        match xai_grok_plugin_marketplace::skill_install::remove(name, target, keep_files) {
            Ok(path) => removed.push(RemovedSkillJson {
                name: name.to_string(),
                scope: target.kind().as_str().to_string(),
                path,
                files_removed: !keep_files,
            }),
            Err(SkillInstallError::NotInstalled(_)) => continue,
            Err(error) => last_error = Some(error),
        }
    }

    if removed.is_empty() {
        return match last_error {
            Some(error) => Err(describe_install_error(error)),
            None => bail!("`{name}` is not installed in {}", scope_label(scope)),
        };
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&removed)?);
    } else {
        for entry in &removed {
            let suffix = if keep_files { " (files kept)" } else { "" };
            println!("  removed {} [{}]{suffix}", entry.name, entry.scope);
        }
    }
    Ok(())
}

// ── Output helpers ──────────────────────────────────────────────────

fn report_installed(
    installed: &[InstalledSkill],
    target: &SkillScopeTarget,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    if json {
        let payload: Vec<InstalledSkillJson> = installed
            .iter()
            .map(|skill| installed_json(skill, target.kind()))
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    for skill in installed {
        let action = if dry_run {
            "would install"
        } else {
            skill.action.as_str()
        };
        println!("  {action}: {} — {}", skill.name, skill.description);
        println!("    {}", skill.path.display());
    }

    if dry_run {
        println!("\nDry run: nothing was written.");
    } else {
        println!(
            "\n{} skill(s) available in the {} scope. Run one with /{}",
            installed.len(),
            target.kind().as_str(),
            installed
                .first()
                .map(|s| s.name.as_str())
                .unwrap_or("<name>")
        );
    }
    Ok(())
}

fn installed_json(skill: &InstalledSkill, kind: SkillScopeKind) -> InstalledSkillJson {
    InstalledSkillJson {
        name: skill.name.clone(),
        description: skill.description.clone(),
        path: skill.path.clone(),
        action: skill.action.as_str().to_string(),
        files: skill.file_count,
        scope: kind.as_str().to_string(),
    }
}

/// Human-readable install counts, so a leaderboard column stays narrow.
fn format_installs(installs: u64) -> String {
    match installs {
        0 => "—".to_string(),
        n if n < 1_000 => n.to_string(),
        n if n < 1_000_000 => format!("{:.1}k", n as f64 / 1_000.0),
        n => format!("{:.1}m", n as f64 / 1_000_000.0),
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let kept: String = value.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// First few lines of a `SKILL.md`, for the `info` preview.
fn preview(contents: &str) -> String {
    contents.lines().take(20).collect::<Vec<_>>().join("\n")
}

fn example_spec(results: &[RegistrySkill]) -> String {
    results
        .first()
        .map(RegistrySkill::install_spec)
        .unwrap_or_else(|| "<owner>/<repo>/<skill>".to_string())
}

fn scope_label(scope: Option<&str>) -> String {
    scope.map_or_else(|| "any scope".to_string(), |s| format!("the {s} scope"))
}

/// Turn a registry failure into advice the user can act on.
fn describe_registry_error(error: RegistryError) -> anyhow::Error {
    match error {
        RegistryError::Transport(detail) => anyhow::anyhow!(
            "could not reach the skill registry ({detail}). Check your network, \
             or set GROK_SKILLS_REGISTRY_URL to a reachable mirror."
        ),
        RegistryError::Status { status, .. } if status >= 500 => anyhow::anyhow!(
            "the skill registry is unavailable right now (HTTP {status}). Try again shortly."
        ),
        other => anyhow::Error::new(other),
    }
}

/// Turn an install failure into advice the user can act on.
fn describe_install_error(error: SkillInstallError) -> anyhow::Error {
    match error {
        SkillInstallError::Registry(inner) => describe_registry_error(inner),
        other => anyhow::Error::new(other),
    }
}

#[cfg(test)]
#[path = "skill_cmd_tests.rs"]
mod tests;
