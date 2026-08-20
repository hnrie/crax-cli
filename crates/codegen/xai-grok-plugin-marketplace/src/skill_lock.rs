//! Install records for marketplace-installed skills.
//!
//! Every installed skill gets a lockfile entry recording where it came from and
//! what its contents hashed to. That makes three things possible without
//! re-reading the network: telling marketplace skills apart from hand-written
//! ones, detecting local edits before an update overwrites them, and skipping
//! updates whose upstream content did not change.
//!
//! The lockfile lives beside the skills it describes — `<scope>/skills.lock.json`
//! next to the `skills/` directory — so a project-scoped lockfile can be
//! committed with the project and a user-scoped one stays personal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Lockfile schema version. A lower version on disk is discarded rather than
/// migrated, since re-installing is cheap and a wrong migration is not.
pub const LOCK_VERSION: u32 = 1;

/// Lockfile name, written next to the `skills/` directory it describes.
pub const LOCK_FILE_NAME: &str = "skills.lock.json";

/// Errors from reading or writing the lockfile.
#[derive(Debug, thiserror::Error)]
pub enum SkillLockError {
    #[error("failed to read skill lockfile at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write skill lockfile at {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize skill lockfile: {0}")]
    Serialize(String),
}

/// How an installed skill was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOrigin {
    /// Downloaded from the skills.sh registry API.
    Registry,
    /// Cloned from a git repository.
    Git,
    /// Copied from a directory on this machine.
    Local,
}

impl SkillOrigin {
    /// Short lowercase label for display and JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Git => "git",
            Self::Local => "local",
        }
    }
}

/// One installed skill's provenance record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLockEntry {
    /// The source string the user originally passed to `install`.
    pub source: String,
    /// Normalized source identity, used to re-resolve on update.
    pub source_id: String,
    /// How the skill was obtained.
    pub origin: SkillOrigin,
    /// Registry id (`source/slug`) when the skill came from the registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_id: Option<String>,
    /// Git ref that was checked out, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Resolved commit sha, when the source was a git repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Content hash of the installed files at install time.
    pub content_hash: String,
    /// Hash the registry reported, when it supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_hash: Option<String>,
    /// RFC 3339 timestamp of the first install.
    pub installed_at: String,
    /// RFC 3339 timestamp of the most recent install or update.
    pub updated_at: String,
}

/// The full lockfile contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLock {
    /// Schema version.
    pub version: u32,
    /// Installed skills, keyed by installed skill name and kept sorted.
    #[serde(default)]
    pub skills: BTreeMap<String, SkillLockEntry>,
}

impl Default for SkillLock {
    fn default() -> Self {
        Self {
            version: LOCK_VERSION,
            skills: BTreeMap::new(),
        }
    }
}

impl SkillLock {
    /// Lockfile path for a scope root (the directory containing `skills/`).
    pub fn path_for(scope_root: &Path) -> PathBuf {
        scope_root.join(LOCK_FILE_NAME)
    }

    /// Load the lockfile for a scope root.
    ///
    /// A missing file, unreadable file, unparsable file, or older schema
    /// version all yield an empty lock: install records are a cache of
    /// provenance, and refusing to run because the cache is damaged would be
    /// worse than rebuilding it.
    pub fn load(scope_root: &Path) -> Self {
        let path = Self::path_for(scope_root);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&raw) {
            Ok(lock) if lock.version == LOCK_VERSION => lock,
            Ok(_) => {
                tracing::debug!(path = %path.display(), "discarding skill lockfile from an older schema");
                Self::default()
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "skill lockfile is unreadable; treating it as empty");
                Self::default()
            }
        }
    }

    /// Write the lockfile, creating the scope root if needed.
    ///
    /// An empty lock removes the file instead of leaving an empty husk behind.
    pub fn save(&self, scope_root: &Path) -> Result<(), SkillLockError> {
        let path = Self::path_for(scope_root);
        if self.skills.is_empty() {
            match std::fs::remove_file(&path) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(source) => return Err(SkillLockError::Write { path, source }),
            }
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SkillLockError::Write {
                path: path.clone(),
                source,
            })?;
        }
        let mut json = serde_json::to_string_pretty(self)
            .map_err(|e| SkillLockError::Serialize(e.to_string()))?;
        json.push('\n');
        std::fs::write(&path, json).map_err(|source| SkillLockError::Write { path, source })
    }

    /// Record an install or update, preserving the original install timestamp.
    pub fn upsert(&mut self, name: &str, mut entry: SkillLockEntry) {
        if let Some(existing) = self.skills.get(name) {
            entry.installed_at = existing.installed_at.clone();
        }
        self.skills.insert(name.to_string(), entry);
    }

    /// Remove a skill's record, reporting whether one existed.
    pub fn remove(&mut self, name: &str) -> bool {
        self.skills.remove(name).is_some()
    }

    /// Look up a skill's record.
    pub fn get(&self, name: &str) -> Option<&SkillLockEntry> {
        self.skills.get(name)
    }

    /// Whether a skill is tracked as marketplace-installed.
    pub fn contains(&self, name: &str) -> bool {
        self.skills.contains_key(name)
    }
}

/// Hash a set of files into a stable content fingerprint.
///
/// Paths are sorted and length-prefixed alongside their contents so that
/// renaming a file, moving content between files, or reordering the input all
/// produce different hashes.
pub fn hash_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    use sha2::{Digest, Sha256};

    let mut entries: Vec<(&str, &[u8])> = files.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let mut hasher = Sha256::new();
    for (path, contents) in entries {
        let normalized = path.replace('\\', "/");
        hasher.update((normalized.len() as u64).to_le_bytes());
        hasher.update(normalized.as_bytes());
        hasher.update((contents.len() as u64).to_le_bytes());
        hasher.update(contents);
    }
    format!("{:x}", hasher.finalize())
}

/// Hash an installed skill directory on disk.
///
/// Used to detect local edits: if the directory no longer hashes to the value
/// recorded at install time, an update would discard the user's changes.
pub fn hash_skill_dir(dir: &Path) -> std::io::Result<String> {
    let mut collected: Vec<(String, Vec<u8>)> = Vec::new();
    collect_files(dir, dir, &mut collected)?;
    Ok(hash_files(
        collected
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice())),
    ))
}

/// Recursively read a directory into `(relative path, contents)` pairs.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((relative, std::fs::read(&path)?));
        }
        // Symlinks are skipped: an installed skill tree is written as plain
        // files, so a link here is either foreign or a traversal attempt.
    }
    Ok(())
}

/// Current time as an RFC 3339 timestamp.
pub fn now_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
#[path = "skill_lock_tests.rs"]
mod tests;
