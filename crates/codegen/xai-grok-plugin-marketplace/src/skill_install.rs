//! Install, update, and remove marketplace skills on disk.
//!
//! A skill install is a directory copy plus a lockfile record. Three source
//! kinds feed it — the skills.sh registry, a git repository, and a local
//! directory — and all three converge on the same staged tree before anything
//! is written into the destination scope:
//!
//! 1. Materialize candidate skills into a staging area (download, clone, or scan).
//! 2. Validate each candidate: `SKILL.md` present, frontmatter parses, name is a
//!    safe slug, destination stays inside the scope.
//! 3. Copy into `<scope>/skills/<name>/` and record provenance in the lockfile.
//!
//! Staging first means a failure partway through never leaves a half-written
//! skill where the agent would load it.

use std::path::{Path, PathBuf};

use crate::skill_lock::{
    SkillLock, SkillLockEntry, SkillOrigin, hash_files, hash_skill_dir, now_timestamp,
};
use crate::skill_registry::{RegistryClient, RegistryError, is_safe_relative_path};
use crate::skill_scope::SkillScopeTarget;
use crate::skill_source::{SkillSource, SkillSourceError};

/// Cap on files copied from a git or local source, matching the registry cap.
const MAX_SKILL_FILES: usize = 2_000;

/// Cap on total bytes copied from a git or local source.
const MAX_SKILL_BYTES: u64 = 32 * 1024 * 1024;

/// How deep to search a source tree for `SKILL.md` files.
const MAX_SCAN_DEPTH: usize = 8;

/// Errors from installing, updating, or removing a skill.
#[derive(Debug, thiserror::Error)]
pub enum SkillInstallError {
    #[error(transparent)]
    Source(#[from] SkillSourceError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Lock(#[from] crate::skill_lock::SkillLockError),
    #[error("no SKILL.md found in {0}")]
    NoSkillsFound(String),
    #[error("skill `{name}` is not valid: {reason}")]
    InvalidSkill { name: String, reason: String },
    #[error(
        "skill `{0}` is already installed; pass --force to overwrite it or run `grok skill update {0}`"
    )]
    AlreadyInstalled(String),
    #[error(
        "skill `{0}` has local edits; pass --force to discard them or copy them out before updating"
    )]
    LocallyModified(String),
    #[error("skill `{0}` is not installed in this scope")]
    NotInstalled(String),
    #[error("skill `{0}` was not installed from the marketplace, so it cannot be updated")]
    NotTracked(String),
    #[error("source `{spec}` does not contain a skill named `{skill}`")]
    SkillNotInSource { spec: String, skill: String },
    #[error("source exceeds the install size limit ({limit} bytes)")]
    TooLarge { limit: u64 },
    #[error("git operation failed: {0}")]
    Git(String),
    #[error("filesystem error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// What an install or update did to one skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallAction {
    /// The skill was newly written into the scope.
    Installed,
    /// The skill was already present and its contents were replaced.
    Updated,
    /// The skill was already present with identical contents.
    Unchanged,
}

impl InstallAction {
    /// Short lowercase label for display and JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
        }
    }
}

/// Outcome of installing or updating a single skill.
#[derive(Debug, Clone)]
pub struct InstalledSkill {
    /// Installed skill name, which is also its slash-command name.
    pub name: String,
    /// Description taken from the skill's frontmatter.
    pub description: String,
    /// Where the skill was written.
    pub path: PathBuf,
    /// What happened to it.
    pub action: InstallAction,
    /// Content hash of the installed files.
    pub content_hash: String,
    /// Number of files written.
    pub file_count: usize,
}

/// Options controlling an install.
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Overwrite an existing skill, including one with local edits.
    pub force: bool,
    /// Validate and report without writing anything.
    pub dry_run: bool,
    /// Install only the skill with this name from a multi-skill source.
    pub only: Option<String>,
}

/// A skill discovered in a staging directory, ready to be validated and copied.
#[derive(Debug, Clone)]
struct StagedSkill {
    name: String,
    description: String,
    dir: PathBuf,
}

/// Install skills from `source` into `scope`.
///
/// A source containing several skills installs all of them unless
/// [`InstallOptions::only`] or the source's own skill filter narrows it.
pub async fn install(
    source: &SkillSource,
    original_spec: &str,
    scope: &SkillScopeTarget,
    options: &InstallOptions,
    registry: &RegistryClient,
) -> Result<Vec<InstalledSkill>, SkillInstallError> {
    let staging = tempfile::tempdir().map_err(|source| SkillInstallError::Io {
        path: std::env::temp_dir(),
        source,
    })?;

    let (candidates, provenance) = match source {
        SkillSource::Registry {
            source: registry_source,
            slug,
        } => stage_registry(registry, registry_source, slug, staging.path()).await?,
        SkillSource::Git {
            url,
            git_ref,
            subpath,
            skill_filter,
            ..
        } => stage_git(
            url,
            git_ref.as_deref(),
            subpath.as_deref(),
            skill_filter.as_deref(),
            staging.path(),
        )?,
        SkillSource::Local { path } => stage_local(path)?,
    };

    let wanted = options
        .only
        .as_deref()
        .map(crate::skill_validate::normalize_name);
    let selected: Vec<StagedSkill> = match &wanted {
        Some(only) => candidates
            .into_iter()
            .filter(|skill| &skill.name == only)
            .collect(),
        None => candidates,
    };

    if selected.is_empty() {
        return match wanted {
            Some(only) => Err(SkillInstallError::SkillNotInSource {
                spec: original_spec.to_string(),
                skill: only,
            }),
            None => Err(SkillInstallError::NoSkillsFound(original_spec.to_string())),
        };
    }

    let mut lock = SkillLock::load(scope.root());
    let mut results = Vec::with_capacity(selected.len());
    let mut failure = None;

    for staged in selected {
        match install_one(
            &staged,
            original_spec,
            source,
            &provenance,
            scope,
            options,
            &mut lock,
        ) {
            Ok(result) => results.push(result),
            Err(error) => {
                // Stop at the first failure, but keep what already landed: the
                // files are on disk either way, so the lockfile has to describe
                // them or a later update would treat them as hand-written.
                failure = Some(error);
                break;
            }
        }
    }

    if !options.dry_run && !results.is_empty() {
        lock.save(scope.root())?;
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(results),
    }
}

/// Extra provenance a staging step resolved that the source string alone lacks.
#[derive(Debug, Clone, Default)]
struct Provenance {
    origin_override: Option<SkillOrigin>,
    commit: Option<String>,
    registry_hash: Option<String>,
    registry_id: Option<String>,
}

/// Install one staged skill into the scope and record it in `lock`.
fn install_one(
    staged: &StagedSkill,
    original_spec: &str,
    source: &SkillSource,
    provenance: &Provenance,
    scope: &SkillScopeTarget,
    options: &InstallOptions,
    lock: &mut SkillLock,
) -> Result<InstalledSkill, SkillInstallError> {
    let destination =
        scope
            .skill_dir(&staged.name)
            .ok_or_else(|| SkillInstallError::InvalidSkill {
                name: staged.name.clone(),
                reason: "resolved install path escapes the skills directory".to_string(),
            })?;

    let files = read_tree(&staged.dir)?;
    let content_hash = hash_files(
        files
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice())),
    );

    let existed = destination.exists();
    if existed {
        let installed_hash = hash_skill_dir(&destination).ok();
        if installed_hash.as_deref() == Some(content_hash.as_str()) {
            // Reinstalling identical contents is a no-op, but the lockfile
            // still learns about a skill that predates it or changed source.
            record(
                lock,
                staged,
                original_spec,
                source,
                provenance,
                &content_hash,
            );
            return Ok(InstalledSkill {
                name: staged.name.clone(),
                description: staged.description.clone(),
                path: destination,
                action: InstallAction::Unchanged,
                content_hash,
                file_count: files.len(),
            });
        }
        if !options.force {
            let tracked = lock.get(&staged.name);
            match tracked {
                None => return Err(SkillInstallError::AlreadyInstalled(staged.name.clone())),
                Some(entry) => {
                    let drifted = installed_hash
                        .as_deref()
                        .is_some_and(|current| current != entry.content_hash);
                    if drifted {
                        return Err(SkillInstallError::LocallyModified(staged.name.clone()));
                    }
                }
            }
        }
    }

    let action = if existed {
        InstallAction::Updated
    } else {
        InstallAction::Installed
    };

    if options.dry_run {
        return Ok(InstalledSkill {
            name: staged.name.clone(),
            description: staged.description.clone(),
            path: destination,
            action,
            content_hash,
            file_count: files.len(),
        });
    }

    write_tree(&destination, &files)?;
    record(
        lock,
        staged,
        original_spec,
        source,
        provenance,
        &content_hash,
    );

    Ok(InstalledSkill {
        name: staged.name.clone(),
        description: staged.description.clone(),
        path: destination,
        action,
        content_hash,
        file_count: files.len(),
    })
}

/// Write a lockfile entry for a freshly installed skill.
fn record(
    lock: &mut SkillLock,
    staged: &StagedSkill,
    original_spec: &str,
    source: &SkillSource,
    provenance: &Provenance,
    content_hash: &str,
) {
    let origin = provenance.origin_override.unwrap_or(match source {
        SkillSource::Registry { .. } => SkillOrigin::Registry,
        SkillSource::Git { .. } => SkillOrigin::Git,
        SkillSource::Local { .. } => SkillOrigin::Local,
    });
    let git_ref = match source {
        SkillSource::Git { git_ref, .. } => git_ref.clone(),
        _ => None,
    };
    let now = now_timestamp();
    lock.upsert(
        &staged.name,
        SkillLockEntry {
            source: original_spec.to_string(),
            source_id: source.identity(),
            origin,
            registry_id: provenance.registry_id.clone(),
            git_ref,
            commit: provenance.commit.clone(),
            content_hash: content_hash.to_string(),
            registry_hash: provenance.registry_hash.clone(),
            installed_at: now.clone(),
            updated_at: now,
        },
    );
}

/// Download a registry skill into `staging`.
async fn stage_registry(
    registry: &RegistryClient,
    source: &str,
    slug: &str,
    staging: &Path,
) -> Result<(Vec<StagedSkill>, Provenance), SkillInstallError> {
    let (owner, repo) = source
        .split_once('/')
        .ok_or_else(|| RegistryError::NotFound(format!("{source}/{slug}")))?;
    let payload = registry.download(owner, repo, slug).await?;

    let dir = staging.join(crate::skill_validate::normalize_name(slug));
    std::fs::create_dir_all(&dir).map_err(|e| SkillInstallError::Io {
        path: dir.clone(),
        source: e,
    })?;

    for file in &payload.files {
        // Registry paths were validated on download; re-check before writing.
        if !is_safe_relative_path(&file.path) {
            return Err(RegistryError::UnsafePath {
                name: format!("{source}/{slug}"),
                path: file.path.clone(),
            }
            .into());
        }
        let target = dir.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SkillInstallError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        std::fs::write(&target, &file.contents).map_err(|e| SkillInstallError::Io {
            path: target.clone(),
            source: e,
        })?;
    }

    let staged = collect_staged(&dir)?;
    let provenance = Provenance {
        origin_override: Some(SkillOrigin::Registry),
        commit: None,
        registry_hash: payload.hash.clone(),
        registry_id: Some(format!("{source}/{slug}")),
    };
    Ok((staged, provenance))
}

/// Clone a git source into `staging` and scan it for skills.
fn stage_git(
    url: &str,
    git_ref: Option<&str>,
    subpath: Option<&str>,
    skill_filter: Option<&str>,
    staging: &Path,
) -> Result<(Vec<StagedSkill>, Provenance), SkillInstallError> {
    let checkout = staging.join("repo");
    let commit =
        crate::skill_git::clone_source(url, git_ref, &checkout).map_err(SkillInstallError::Git)?;

    let scan_root = match subpath {
        Some(sub) => {
            let joined = crate::skill_validate::join_within(&checkout, sub).ok_or_else(|| {
                SkillInstallError::InvalidSkill {
                    name: sub.to_string(),
                    reason: "subdirectory escapes the repository root".to_string(),
                }
            })?;
            if !joined.is_dir() {
                return Err(SkillInstallError::NoSkillsFound(format!("{url} ({sub})")));
            }
            joined
        }
        None => checkout.clone(),
    };

    let mut staged = collect_staged(&scan_root)?;
    if let Some(filter) = skill_filter {
        let wanted = crate::skill_validate::normalize_name(filter);
        staged.retain(|skill| skill.name == wanted);
        if staged.is_empty() {
            return Err(SkillInstallError::SkillNotInSource {
                spec: url.to_string(),
                skill: wanted,
            });
        }
    }

    let provenance = Provenance {
        origin_override: Some(SkillOrigin::Git),
        commit,
        registry_hash: None,
        registry_id: None,
    };
    Ok((staged, provenance))
}

/// Scan a local directory for skills without copying it first.
fn stage_local(path: &Path) -> Result<(Vec<StagedSkill>, Provenance), SkillInstallError> {
    if !path.exists() {
        return Err(SkillInstallError::NoSkillsFound(path.display().to_string()));
    }
    let staged = collect_staged(path)?;
    let provenance = Provenance {
        origin_override: Some(SkillOrigin::Local),
        ..Provenance::default()
    };
    Ok((staged, provenance))
}

/// Find every valid skill under `root`.
///
/// A directory that holds a `SKILL.md` is a skill; its subdirectories are part
/// of that skill rather than nested skills, which matches how skill authors
/// package `rules/` and `scripts/` alongside the entry point.
fn collect_staged(root: &Path) -> Result<Vec<StagedSkill>, SkillInstallError> {
    let mut found = Vec::new();
    scan_dir(root, root, 0, &mut found)?;
    if found.is_empty() {
        return Err(SkillInstallError::NoSkillsFound(root.display().to_string()));
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|a, b| a.name == b.name);
    Ok(found)
}

/// Recursive half of [`collect_staged`].
fn scan_dir(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<StagedSkill>,
) -> Result<(), SkillInstallError> {
    if depth > MAX_SCAN_DEPTH {
        return Ok(());
    }

    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() {
        let contents =
            std::fs::read_to_string(&skill_md).map_err(|source| SkillInstallError::Io {
                path: skill_md.clone(),
                source,
            })?;
        let fallback = if dir == root {
            root.file_name().and_then(|n| n.to_str())
        } else {
            dir.file_name().and_then(|n| n.to_str())
        };
        match crate::skill_validate::validate_skill_md(&contents, fallback) {
            Ok(valid) => {
                out.push(StagedSkill {
                    name: valid.name,
                    description: valid.description,
                    dir: dir.to_path_buf(),
                });
                // Everything below belongs to this skill.
                return Ok(());
            }
            Err(reason) => {
                tracing::warn!(path = %skill_md.display(), %reason, "skipping invalid SKILL.md");
                return Ok(());
            }
        }
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Symlinks are not followed: a link inside a cloned repository can
        // point anywhere on the host filesystem.
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if is_skipped_dir(&name) {
            continue;
        }
        children.push(path);
    }
    children.sort();
    for child in children {
        scan_dir(root, &child, depth + 1, out)?;
    }
    Ok(())
}

/// Directories that never contain installable skills.
fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".github"
            | "node_modules"
            | "target"
            | "__pycache__"
            | "__pypackages__"
            | ".venv"
            | "venv"
            | ".next"
            | "dist"
            | "build"
    )
}

/// Read a staged skill directory into `(relative path, bytes)` pairs.
fn read_tree(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, SkillInstallError> {
    let mut files = Vec::new();
    let mut total: u64 = 0;
    read_tree_inner(dir, dir, &mut files, &mut total)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Recursive half of [`read_tree`], enforcing the size and count limits.
fn read_tree_inner(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
    total: &mut u64,
) -> Result<(), SkillInstallError> {
    let entries = std::fs::read_dir(dir).map_err(|source| SkillInstallError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            if is_skipped_dir(&name.to_string_lossy()) {
                continue;
            }
            read_tree_inner(root, &path, out, total)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if !is_safe_relative_path(&relative) || relative == "metadata.json" {
            continue;
        }

        let bytes = std::fs::read(&path).map_err(|source| SkillInstallError::Io {
            path: path.clone(),
            source,
        })?;
        *total = total.saturating_add(bytes.len() as u64);
        if *total > MAX_SKILL_BYTES {
            return Err(SkillInstallError::TooLarge {
                limit: MAX_SKILL_BYTES,
            });
        }
        out.push((relative, bytes));
        if out.len() > MAX_SKILL_FILES {
            return Err(SkillInstallError::TooLarge {
                limit: MAX_SKILL_FILES as u64,
            });
        }
    }
    Ok(())
}

/// Replace `destination` with `files`, writing through a sibling temp dir so a
/// failure cannot leave a partially written skill in place.
fn write_tree(destination: &Path, files: &[(String, Vec<u8>)]) -> Result<(), SkillInstallError> {
    let parent = destination
        .parent()
        .ok_or_else(|| SkillInstallError::InvalidSkill {
            name: destination.display().to_string(),
            reason: "install path has no parent directory".to_string(),
        })?;
    std::fs::create_dir_all(parent).map_err(|source| SkillInstallError::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let staging = tempfile::TempDir::new_in(parent).map_err(|source| SkillInstallError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let staged_root = staging.path().join("skill");
    std::fs::create_dir_all(&staged_root).map_err(|source| SkillInstallError::Io {
        path: staged_root.clone(),
        source,
    })?;

    for (relative, bytes) in files {
        let target =
            crate::skill_validate::join_within(&staged_root, relative).ok_or_else(|| {
                SkillInstallError::InvalidSkill {
                    name: relative.clone(),
                    reason: "file path escapes the skill directory".to_string(),
                }
            })?;
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|source| SkillInstallError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&target, bytes).map_err(|source| SkillInstallError::Io {
            path: target.clone(),
            source,
        })?;
    }

    if destination.exists() {
        std::fs::remove_dir_all(destination).map_err(|source| SkillInstallError::Io {
            path: destination.to_path_buf(),
            source,
        })?;
    }
    // A rename across the same parent is atomic on both Unix and Windows;
    // fall back to a copy when the platform refuses (for example a
    // cross-device staging dir).
    match std::fs::rename(&staged_root, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_dir(&staged_root, destination)?;
            Ok(())
        }
    }
}

/// Recursive directory copy used as the non-atomic fallback path.
fn copy_dir(from: &Path, to: &Path) -> Result<(), SkillInstallError> {
    std::fs::create_dir_all(to).map_err(|source| SkillInstallError::Io {
        path: to.to_path_buf(),
        source,
    })?;
    let entries = std::fs::read_dir(from).map_err(|source| SkillInstallError::Io {
        path: from.to_path_buf(),
        source,
    })?;
    for entry in entries.flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            copy_dir(&src, &dst)?;
        } else if file_type.is_file() {
            std::fs::copy(&src, &dst).map_err(|source| SkillInstallError::Io {
                path: dst.clone(),
                source,
            })?;
        }
    }
    Ok(())
}

/// Remove an installed skill and its lockfile record.
///
/// Removal works for hand-written skills too, so a user is never stuck with a
/// skill directory the CLI refuses to touch. `keep_files` drops only the
/// tracking record.
pub fn remove(
    name: &str,
    scope: &SkillScopeTarget,
    keep_files: bool,
) -> Result<PathBuf, SkillInstallError> {
    let normalized = crate::skill_validate::normalize_name(name);
    let dir = scope
        .skill_dir(&normalized)
        .ok_or_else(|| SkillInstallError::NotInstalled(name.to_string()))?;

    let mut lock = SkillLock::load(scope.root());
    let tracked = lock.remove(&normalized);

    if !dir.exists() {
        if !tracked {
            return Err(SkillInstallError::NotInstalled(name.to_string()));
        }
        // The directory was deleted by hand; clearing the stale record is
        // still the right outcome.
        lock.save(scope.root())?;
        return Ok(dir);
    }

    if !keep_files {
        std::fs::remove_dir_all(&dir).map_err(|source| SkillInstallError::Io {
            path: dir.clone(),
            source,
        })?;
    }
    lock.save(scope.root())?;
    Ok(dir)
}

/// Re-resolve a tracked skill from its recorded source and reinstall it.
pub async fn update(
    name: &str,
    scope: &SkillScopeTarget,
    options: &InstallOptions,
    registry: &RegistryClient,
    cwd: &Path,
) -> Result<InstalledSkill, SkillInstallError> {
    let normalized = crate::skill_validate::normalize_name(name);
    let lock = SkillLock::load(scope.root());
    let entry = lock
        .get(&normalized)
        .ok_or_else(|| SkillInstallError::NotTracked(name.to_string()))?
        .clone();

    let source = crate::skill_source::parse_skill_source(
        &entry.source,
        cwd,
        entry.origin == SkillOrigin::Registry,
    )?;

    let mut options = options.clone();
    // An update targets exactly the skill being updated, even when its source
    // repository contains several.
    options.only = Some(normalized.clone());

    let results = install(&source, &entry.source, scope, &options, registry).await?;
    results
        .into_iter()
        .find(|installed| installed.name == normalized)
        .ok_or(SkillInstallError::SkillNotInSource {
            spec: entry.source,
            skill: normalized,
        })
}

#[cfg(test)]
#[path = "skill_install_tests.rs"]
mod tests;
