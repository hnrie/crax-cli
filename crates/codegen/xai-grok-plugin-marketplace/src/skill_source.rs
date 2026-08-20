//! Parse a user-supplied skill source string into a resolvable specification.
//!
//! Accepts the same shapes the skills.sh ecosystem uses, so a source copied
//! from a browser address bar, a README badge, or a registry listing resolves
//! without reformatting:
//!
//! - `owner/repo` GitHub shorthand, optionally `owner/repo/subdir`
//! - `owner/repo@skill-name` to install a single skill out of a repo
//! - `https://github.com/owner/repo`, `.../tree/<ref>`, `.../tree/<ref>/<subdir>`
//! - `github:owner/repo`, `gitlab:group/project`, `git@github.com:owner/repo.git`
//! - `https://gitlab.com/group/project/-/tree/<ref>/<subdir>`
//! - `#<ref>` / `#<ref>@<skill>` fragments on any git form
//! - local filesystem paths (`.`, `./skills`, `/abs/path`, `~/skills`)
//! - registry ids (`owner/repo/skill-slug` that the registry resolves directly)
//!
//! Parsing is intentionally pure: no network access and no filesystem writes,
//! so callers can validate and display a plan before committing to it.

use std::path::{Path, PathBuf};

/// Where a skill install should read its files from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// A directory on this machine that contains one or more `SKILL.md` files.
    Local { path: PathBuf },
    /// A git repository that must be cloned before scanning.
    Git {
        /// Clone URL, always normalized to an explicit `.git` form when known.
        url: String,
        /// Branch, tag, or commit to check out. `None` uses the default branch.
        git_ref: Option<String>,
        /// Directory inside the repository to scan instead of the root.
        subpath: Option<String>,
        /// Install only the named skill from the repository.
        skill_filter: Option<String>,
        /// Host family, used for display and for registry id derivation.
        host: GitHost,
    },
    /// A skill hosted by the skills.sh registry, addressed as `owner/repo/slug`.
    Registry {
        /// `owner/repo` (GitHub sources) or `domain.com` (well-known sources).
        source: String,
        /// The skill slug within that source.
        slug: String,
    },
}

/// Git hosting family for a parsed source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHost {
    GitHub,
    GitLab,
    Other,
}

impl GitHost {
    /// Short lowercase label for display and JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
            Self::Other => "git",
        }
    }
}

impl SkillSource {
    /// Stable identity used as the lockfile key prefix and for dedup.
    pub fn identity(&self) -> String {
        match self {
            Self::Local { path } => format!("local:{}", path.display()),
            Self::Git {
                url,
                git_ref,
                subpath,
                ..
            } => {
                let mut id = url.clone();
                if let Some(r) = git_ref {
                    id.push('#');
                    id.push_str(r);
                }
                if let Some(sub) = subpath {
                    id.push('/');
                    id.push_str(sub);
                }
                id
            }
            Self::Registry { source, slug } => format!("{source}/{slug}"),
        }
    }

    /// Short label describing the source kind, for listings and JSON.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local",
            Self::Git { host, .. } => host.as_str(),
            Self::Registry { .. } => "registry",
        }
    }
}

/// Errors produced while parsing a source string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillSourceError {
    #[error("skill source is empty")]
    Empty,
    #[error("skill source `{0}` is not a recognized repository, URL, or path")]
    Unrecognized(String),
    #[error("skill source `{0}` contains a path component that escapes the repository")]
    UnsafeSubpath(String),
    #[error("skill source `{0}` contains a control character")]
    ControlCharacter(String),
}

/// Parse `input` into a [`SkillSource`], resolving relative paths against `cwd`.
///
/// `prefer_registry` selects how a bare three-segment `owner/repo/slug` is read.
/// The registry addresses skills that way, while git shorthand reads the third
/// segment as a subdirectory; both are valid so the caller picks the meaning.
pub fn parse_skill_source(
    input: &str,
    cwd: &Path,
    prefer_registry: bool,
) -> Result<SkillSource, SkillSourceError> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(SkillSourceError::Empty);
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err(SkillSourceError::ControlCharacter(raw.to_string()));
    }

    if let Some(path) = local_path(raw, cwd) {
        return Ok(SkillSource::Local { path });
    }

    let (body, frag_ref, frag_skill) = split_fragment(raw);

    if let Some(rest) = body.strip_prefix("github:") {
        return parse_skill_source(&rejoin(rest, frag_ref, frag_skill), cwd, false);
    }
    if let Some(rest) = body.strip_prefix("gitlab:") {
        let url = format!("https://gitlab.com/{rest}");
        return parse_skill_source(&rejoin(&url, frag_ref, frag_skill), cwd, false);
    }

    if let Some(rest) = body.strip_prefix("git@") {
        // scp-like syntax: git@host:owner/repo.git
        if let Some((host, path)) = rest.split_once(':') {
            let path = path.trim_end_matches('/').trim_end_matches(".git");
            return Ok(SkillSource::Git {
                url: format!("https://{host}/{path}.git"),
                git_ref: frag_ref.map(str::to_string),
                subpath: None,
                skill_filter: frag_skill.map(str::to_string),
                host: host_family(host),
            });
        }
    }

    if body.starts_with("http://") || body.starts_with("https://") || body.starts_with("ssh://") {
        return parse_url(body, frag_ref, frag_skill);
    }

    parse_shorthand(body, frag_ref, frag_skill, prefer_registry)
}

/// Split a trailing `#ref` or `#ref@skill` fragment off a source string.
fn split_fragment(input: &str) -> (&str, Option<&str>, Option<&str>) {
    let Some(hash) = input.find('#') else {
        return (input, None, None);
    };
    let (body, fragment) = input.split_at(hash);
    let fragment = &fragment[1..];
    if body.is_empty() || fragment.is_empty() {
        return (input, None, None);
    }
    match fragment.split_once('@') {
        Some((git_ref, skill)) => (
            body,
            (!git_ref.is_empty()).then_some(git_ref),
            (!skill.is_empty()).then_some(skill),
        ),
        None => (body, Some(fragment), None),
    }
}

/// Reattach a fragment when recursing through a `github:`/`gitlab:` prefix.
fn rejoin(body: &str, git_ref: Option<&str>, skill: Option<&str>) -> String {
    match (git_ref, skill) {
        (Some(r), Some(s)) => format!("{body}#{r}@{s}"),
        (Some(r), None) => format!("{body}#{r}"),
        (None, Some(s)) => format!("{body}#@{s}"),
        (None, None) => body.to_string(),
    }
}

/// Recognize local filesystem sources, expanding `~` and relative segments.
fn local_path(input: &str, cwd: &Path) -> Option<PathBuf> {
    if input == "." || input == ".." {
        return Some(cwd.join(input));
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return dirs::home_dir().map(|home| home.join(rest));
    }
    if input == "~" {
        return dirs::home_dir();
    }
    if input.starts_with("./") || input.starts_with("../") {
        return Some(cwd.join(input));
    }
    // Windows drive-letter paths (`C:\skills`) and Unix absolute paths.
    let path = Path::new(input);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    if input.starts_with(".\\") || input.starts_with("..\\") {
        return Some(cwd.join(input));
    }
    None
}

/// Map a hostname to its hosting family.
fn host_family(host: &str) -> GitHost {
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if host == "github.com" || host.ends_with(".github.com") {
        GitHost::GitHub
    } else if host == "gitlab.com" || host.ends_with(".gitlab.com") {
        GitHost::GitLab
    } else {
        GitHost::Other
    }
}

/// Reject subpaths that traverse outside the repository root.
fn sanitize_subpath(raw: &str) -> Result<Option<String>, SkillSourceError> {
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(None);
    }
    let mut parts = Vec::new();
    for segment in trimmed.split(['/', '\\']) {
        match segment {
            "" | "." => continue,
            ".." => return Err(SkillSourceError::UnsafeSubpath(raw.to_string())),
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(parts.join("/")))
}

/// Parse an `http(s)`/`ssh` URL into a git source.
fn parse_url(
    body: &str,
    frag_ref: Option<&str>,
    frag_skill: Option<&str>,
) -> Result<SkillSource, SkillSourceError> {
    let (scheme, rest) = body
        .split_once("://")
        .ok_or_else(|| SkillSourceError::Unrecognized(body.to_string()))?;
    let rest = rest.strip_prefix("git@").unwrap_or(rest);
    let (host, path) = rest
        .split_once('/')
        .ok_or_else(|| SkillSourceError::Unrecognized(body.to_string()))?;
    let host_lower = host.to_ascii_lowercase();
    let host_lower = host_lower.strip_prefix("www.").unwrap_or(&host_lower);
    let family = host_family(host);
    // ssh:// clone URLs stay on ssh; everything else is normalized to https.
    let url_scheme = if scheme == "ssh" { "ssh" } else { "https" };

    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() < 2 {
        return Err(SkillSourceError::Unrecognized(body.to_string()));
    }

    // GitLab nests groups, and marks refs with a `/-/tree/` separator.
    if family == GitHost::GitLab
        && let Some(marker) = segments.iter().position(|s| *s == "-")
        && segments.get(marker + 1) == Some(&"tree")
        && marker >= 2
    {
        let repo_path = segments[..marker].join("/");
        let git_ref = segments.get(marker + 2).map(|s| (*s).to_string());
        let subpath = if segments.len() > marker + 3 {
            sanitize_subpath(&segments[marker + 3..].join("/"))?
        } else {
            None
        };
        return Ok(SkillSource::Git {
            url: format!("{url_scheme}://{host_lower}/{repo_path}.git"),
            git_ref: git_ref.or_else(|| frag_ref.map(str::to_string)),
            subpath,
            skill_filter: frag_skill.map(str::to_string),
            host: family,
        });
    }

    let owner = segments[0];
    let repo = segments[1].trim_end_matches(".git");
    let base = format!("{url_scheme}://{host_lower}/{owner}/{repo}.git");

    // `/tree/<ref>` and `/tree/<ref>/<subdir>` browse URLs.
    if segments.get(2) == Some(&"tree")
        && let Some(git_ref) = segments.get(3)
    {
        let subpath = if segments.len() > 4 {
            sanitize_subpath(&segments[4..].join("/"))?
        } else {
            None
        };
        return Ok(SkillSource::Git {
            url: base,
            git_ref: Some((*git_ref).to_string()),
            subpath,
            skill_filter: frag_skill.map(str::to_string),
            host: family,
        });
    }

    let subpath = if segments.len() > 2 {
        sanitize_subpath(&segments[2..].join("/"))?
    } else {
        None
    };
    Ok(SkillSource::Git {
        url: base,
        git_ref: frag_ref.map(str::to_string),
        subpath,
        skill_filter: frag_skill.map(str::to_string),
        host: family,
    })
}

/// Parse bare `owner/repo`-style shorthand.
fn parse_shorthand(
    body: &str,
    frag_ref: Option<&str>,
    frag_skill: Option<&str>,
    prefer_registry: bool,
) -> Result<SkillSource, SkillSourceError> {
    // `owner/repo@skill` selects a single skill from a repository.
    if let Some((head, skill)) = body.rsplit_once('@')
        && !head.is_empty()
        && !skill.is_empty()
        && !skill.contains('/')
        && head.matches('/').count() == 1
    {
        let (owner, repo) = head.split_once('/').expect("checked one separator");
        if is_segment(owner) && is_segment(repo) {
            return Ok(SkillSource::Git {
                url: format!("https://github.com/{owner}/{}.git", trim_git(repo)),
                git_ref: frag_ref.map(str::to_string),
                subpath: None,
                skill_filter: Some(frag_skill.unwrap_or(skill).to_string()),
                host: GitHost::GitHub,
            });
        }
    }

    let segments: Vec<&str> = body
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    // `owner` and `repo` are interpolated into a clone URL, so they must be
    // plain segments. Anything beyond them is a subpath and gets the stricter
    // traversal check, which reports the more specific error.
    if segments.len() < 2 || !segments[..2].iter().all(|s| is_segment(s)) {
        return Err(SkillSourceError::Unrecognized(body.to_string()));
    }
    if segments[2..].contains(&"..") {
        return Err(SkillSourceError::UnsafeSubpath(body.to_string()));
    }
    if !segments[2..].iter().all(|s| is_segment(s)) {
        return Err(SkillSourceError::Unrecognized(body.to_string()));
    }

    // A three-segment shorthand is ambiguous: `owner/repo/slug` is how the
    // registry names a skill, and `owner/repo/subdir` is how git shorthand
    // names a directory. Registry lookups only make sense without a ref.
    if prefer_registry && segments.len() == 3 && frag_ref.is_none() && frag_skill.is_none() {
        return Ok(SkillSource::Registry {
            source: format!("{}/{}", segments[0], segments[1]),
            slug: segments[2].to_string(),
        });
    }

    let owner = segments[0];
    let repo = trim_git(segments[1]);
    let subpath = if segments.len() > 2 {
        sanitize_subpath(&segments[2..].join("/"))?
    } else {
        None
    };
    Ok(SkillSource::Git {
        url: format!("https://github.com/{owner}/{repo}.git"),
        git_ref: frag_ref.map(str::to_string),
        subpath,
        skill_filter: frag_skill.map(str::to_string),
        host: GitHost::GitHub,
    })
}

fn trim_git(repo: &str) -> &str {
    repo.strip_suffix(".git").unwrap_or(repo)
}

/// Whether a shorthand path segment is safe to interpolate into a clone URL.
fn is_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
#[path = "skill_source_tests.rs"]
mod tests;
