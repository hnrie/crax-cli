//! Client for the public skills.sh skill registry.
//!
//! Two endpoints back the whole browse-and-install experience and neither
//! requires credentials:
//!
//! - `GET /api/search?q=<query>&limit=<n>[&owner=<owner>]` — ranked skill hits
//! - `GET /api/download/<owner>/<repo>/<slug>` — the skill's file tree
//!
//! `GROK_SKILLS_REGISTRY_URL` overrides the base URL so an air-gapped or
//! self-hosted mirror can serve the same shapes. Every request is bounded by a
//! timeout because registry availability must never block the CLI.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default registry origin. Overridden by `GROK_SKILLS_REGISTRY_URL`.
pub const DEFAULT_REGISTRY_URL: &str = "https://skills.sh";

/// Environment variable that points the client at a different registry.
pub const REGISTRY_URL_ENV: &str = "GROK_SKILLS_REGISTRY_URL";

/// Upper bound on results the registry search endpoint will return.
pub const MAX_SEARCH_LIMIT: usize = 200;

/// Cap on a single downloaded file, mirroring the registry's own 2 MB limit.
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// Cap on the total size of one downloaded skill.
const MAX_SKILL_BYTES: usize = 32 * 1024 * 1024;

/// Cap on how many files one skill may contain.
const MAX_SKILL_FILES: usize = 2_000;

/// Errors returned by registry operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("search query must be at least 2 characters")]
    QueryTooShort,
    #[error("skill `{0}` was not found in the registry")]
    NotFound(String),
    #[error("registry request failed: {0}")]
    Transport(String),
    #[error("registry returned HTTP {status} for {url}")]
    Status { status: u16, url: String },
    #[error("registry returned a malformed response: {0}")]
    Malformed(String),
    #[error("skill `{name}` exceeds the {limit} byte download limit")]
    TooLarge { name: String, limit: usize },
    #[error("skill `{name}` contains an unsafe file path `{path}`")]
    UnsafePath { name: String, path: String },
}

/// One skill returned by a registry search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySkill {
    /// Stable `{source}/{slug}` identifier.
    pub id: String,
    /// URL-safe slug within its source.
    pub slug: String,
    /// Human-readable name.
    pub name: String,
    /// Owning repository (`owner/repo`) or well-known domain.
    pub source: String,
    /// Total deduplicated install count.
    pub installs: u64,
}

impl RegistrySkill {
    /// The skill's page on the registry website.
    pub fn page_url(&self) -> String {
        format!("{}/{}", registry_base_url(), self.id)
    }

    /// The source string a user would pass to `grok skill install`.
    pub fn install_spec(&self) -> String {
        self.id.clone()
    }
}

/// A single file inside a downloaded skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryFile {
    /// Path relative to the skill root, always using `/` separators.
    pub path: String,
    /// Full text contents.
    pub contents: String,
}

/// A skill's complete file tree as served by the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySkillFiles {
    /// Files that make up the skill.
    pub files: Vec<RegistryFile>,
    /// Content hash the registry computed, used to detect upstream changes.
    #[serde(default)]
    pub hash: Option<String>,
}

impl RegistrySkillFiles {
    /// Locate the skill's `SKILL.md`, which the registry always includes.
    pub fn skill_md(&self) -> Option<&RegistryFile> {
        self.files
            .iter()
            .find(|f| f.path.eq_ignore_ascii_case("SKILL.md"))
    }

    /// Total byte size of all files.
    pub fn total_bytes(&self) -> usize {
        self.files.iter().map(|f| f.contents.len()).sum()
    }
}

/// Raw search payload. Field names follow the registry's JSON.
#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    skills: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    source: String,
    #[serde(default, alias = "skillId")]
    skill_id: Option<String>,
    #[serde(default)]
    installs: Option<u64>,
}

/// Resolve the registry origin, honoring the override environment variable.
pub fn registry_base_url() -> String {
    std::env::var(REGISTRY_URL_ENV)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string())
}

/// Async HTTP client for registry calls.
///
/// The registry is queried from short-lived CLI subcommands, so a per-call
/// client with an explicit timeout keeps failures fast and local.
#[derive(Debug, Clone)]
pub struct RegistryClient {
    base_url: String,
    timeout: Duration,
}

impl Default for RegistryClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RegistryClient {
    /// Client pointed at the configured registry with the default timeout.
    pub fn new() -> Self {
        Self {
            base_url: registry_base_url(),
            timeout: Duration::from_secs(20),
        }
    }

    /// Client pointed at an explicit base URL, for tests and mirrors.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            timeout: Duration::from_secs(20),
        }
    }

    /// Override the per-request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The base URL this client talks to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Search the registry, returning hits ordered by install count.
    ///
    /// `owner` restricts results to one GitHub owner across its repositories.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        owner: Option<&str>,
    ) -> Result<Vec<RegistrySkill>, RegistryError> {
        let query = query.trim();
        if query.chars().count() < 2 {
            return Err(RegistryError::QueryTooShort);
        }
        let limit = limit.clamp(1, MAX_SEARCH_LIMIT);

        let mut url = format!(
            "{}/api/search?q={}&limit={limit}",
            self.base_url,
            urlencoding::encode(query)
        );
        if let Some(owner) = owner.map(str::trim).filter(|o| !o.is_empty()) {
            url.push_str("&owner=");
            url.push_str(&urlencoding::encode(owner));
        }

        let body = self.get_text(&url).await?;
        let parsed: SearchResponse = serde_json::from_str(&body)
            .map_err(|e| RegistryError::Malformed(format!("search response: {e}")))?;

        let mut skills: Vec<RegistrySkill> = parsed
            .skills
            .into_iter()
            .filter_map(|hit| {
                let id = hit.id.trim();
                if id.is_empty() {
                    return None;
                }
                let slug = hit
                    .skill_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| id.rsplit('/').next().map(str::to_string))?;
                let name = if hit.name.trim().is_empty() {
                    slug.clone()
                } else {
                    hit.name.trim().to_string()
                };
                Some(RegistrySkill {
                    id: id.to_string(),
                    slug,
                    name,
                    source: hit.source.trim().to_string(),
                    installs: hit.installs.unwrap_or(0),
                })
            })
            .collect();

        skills.sort_by(|a, b| b.installs.cmp(&a.installs).then_with(|| a.id.cmp(&b.id)));
        Ok(skills)
    }

    /// Download a skill's file tree by `owner`, `repo`, and `slug`.
    pub async fn download(
        &self,
        owner: &str,
        repo: &str,
        slug: &str,
    ) -> Result<RegistrySkillFiles, RegistryError> {
        let name = format!("{owner}/{repo}/{slug}");
        let url = format!(
            "{}/api/download/{}/{}/{}",
            self.base_url,
            urlencoding::encode(owner),
            urlencoding::encode(repo),
            urlencoding::encode(slug)
        );

        let body = match self.get_text(&url).await {
            Ok(body) => body,
            Err(RegistryError::Status { status: 404, .. }) => {
                return Err(RegistryError::NotFound(name));
            }
            Err(other) => return Err(other),
        };

        let files: RegistrySkillFiles = serde_json::from_str(&body)
            .map_err(|e| RegistryError::Malformed(format!("download response: {e}")))?;
        validate_skill_files(&name, &files)?;
        Ok(files)
    }

    /// Download a skill addressed by its registry id (`source/slug`).
    pub async fn download_by_id(&self, id: &str) -> Result<RegistrySkillFiles, RegistryError> {
        let (owner, repo, slug) =
            split_registry_id(id).ok_or_else(|| RegistryError::NotFound(id.trim().to_string()))?;
        self.download(&owner, &repo, &slug).await
    }

    /// Issue a bounded GET and return the response body as text.
    async fn get_text(&self, url: &str) -> Result<String, RegistryError> {
        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(concat!(
                "grok-skill-marketplace/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|e| RegistryError::Transport(e.to_string()))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| RegistryError::Transport(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(RegistryError::Status {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }

        response
            .text()
            .await
            .map_err(|e| RegistryError::Transport(e.to_string()))
    }
}

/// Split a registry id into `(owner, repo, slug)`.
///
/// Well-known sources are a single domain segment (`mintlify.com/mintlify`),
/// so the repo half is empty in that case and the caller's URL collapses to
/// the two-segment form the registry expects.
pub fn split_registry_id(id: &str) -> Option<(String, String, String)> {
    let parts: Vec<&str> = id
        .trim()
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    match parts.as_slice() {
        [owner, repo, slug] => Some((
            (*owner).to_string(),
            (*repo).to_string(),
            (*slug).to_string(),
        )),
        _ => None,
    }
}

/// Reject payloads that are oversized or contain unsafe paths before any of
/// their contents reach the filesystem.
fn validate_skill_files(name: &str, files: &RegistrySkillFiles) -> Result<(), RegistryError> {
    if files.files.is_empty() {
        return Err(RegistryError::Malformed(format!(
            "skill `{name}` contains no files"
        )));
    }
    if files.files.len() > MAX_SKILL_FILES {
        return Err(RegistryError::TooLarge {
            name: name.to_string(),
            limit: MAX_SKILL_FILES,
        });
    }
    let mut total = 0usize;
    for file in &files.files {
        if !is_safe_relative_path(&file.path) {
            return Err(RegistryError::UnsafePath {
                name: name.to_string(),
                path: file.path.clone(),
            });
        }
        if file.contents.len() > MAX_FILE_BYTES {
            return Err(RegistryError::TooLarge {
                name: name.to_string(),
                limit: MAX_FILE_BYTES,
            });
        }
        total = total.saturating_add(file.contents.len());
        if total > MAX_SKILL_BYTES {
            return Err(RegistryError::TooLarge {
                name: name.to_string(),
                limit: MAX_SKILL_BYTES,
            });
        }
    }
    if files.skill_md().is_none() {
        return Err(RegistryError::Malformed(format!(
            "skill `{name}` has no SKILL.md"
        )));
    }
    Ok(())
}

/// Whether a registry-supplied path stays inside the skill directory.
pub fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.chars().any(|c| c.is_control()) {
        return false;
    }
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains(':') {
        return false;
    }
    normalized
        .split('/')
        .all(|segment| !matches!(segment, "" | "." | ".."))
}

#[cfg(test)]
#[path = "skill_registry_tests.rs"]
mod tests;
