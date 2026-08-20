//! Resolve where a marketplace skill should be installed.
//!
//! Skills install into one of two scopes, matching the discovery order the
//! agent already uses:
//!
//! - **user** — `~/.grok/skills/`, available in every project
//! - **project** — `<repo root or cwd>/.grok/skills/`, shared through version control
//!
//! Both scopes keep their lockfile next to the `skills/` directory, so a
//! project's install records travel with the project.

use std::path::{Path, PathBuf};

/// Which scope an install targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillScopeKind {
    /// `~/.grok/skills`, available across all projects.
    #[default]
    User,
    /// `<project>/.grok/skills`, shared with the repository.
    Project,
}

impl SkillScopeKind {
    /// Short lowercase label for display and JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }

    /// Parse a `--scope` value.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "user" | "global" => Some(Self::User),
            "project" | "repo" | "local" => Some(Self::Project),
            _ => None,
        }
    }
}

/// A resolved install location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillScopeTarget {
    kind: SkillScopeKind,
    /// The `.grok` directory that owns this scope.
    root: PathBuf,
}

impl SkillScopeTarget {
    /// Build a target from an already-resolved `.grok` directory.
    pub fn new(kind: SkillScopeKind, root: PathBuf) -> Self {
        Self { kind, root }
    }

    /// Resolve the user scope from the grok home directory.
    pub fn user(grok_home: &Path) -> Self {
        Self::new(SkillScopeKind::User, grok_home.to_path_buf())
    }

    /// Resolve the project scope, preferring the repository root over `cwd`.
    ///
    /// Installing at the repository root rather than a nested directory means
    /// the skill is visible from anywhere in the project, which is what a
    /// version-controlled team skill needs.
    pub fn project(cwd: &Path) -> Self {
        let base = find_repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
        Self::new(SkillScopeKind::Project, base.join(".grok"))
    }

    /// Which scope this target represents.
    pub fn kind(&self) -> SkillScopeKind {
        self.kind
    }

    /// The `.grok` directory that owns this scope; also holds the lockfile.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `skills/` directory inside this scope.
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    /// Absolute path for an installed skill, or `None` when `name` would
    /// escape the skills directory.
    pub fn skill_dir(&self, name: &str) -> Option<PathBuf> {
        let normalized = crate::skill_validate::normalize_name(name);
        if !crate::skill_validate::is_valid_name(&normalized) {
            return None;
        }
        crate::skill_validate::join_within(&self.skills_dir(), &normalized)
    }

    /// Names of the skills currently present in this scope, sorted.
    pub fn installed_names(&self) -> Vec<String> {
        let dir = self.skills_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.path().join("SKILL.md").is_file())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect();
        names.sort();
        names
    }
}

/// Walk up from `start` looking for a `.git` entry.
///
/// A worktree or submodule records `.git` as a file rather than a directory,
/// so both are accepted.
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

#[cfg(test)]
#[path = "skill_scope_tests.rs"]
mod tests;
