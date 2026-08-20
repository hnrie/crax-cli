//! Validate skill names, `SKILL.md` frontmatter, and destination paths.
//!
//! Everything a marketplace install writes passes through here first. A skill
//! name becomes a directory name and a slash command, so it has to be a safe
//! slug; a `SKILL.md` without a usable name or description would install as a
//! skill the agent can never sensibly invoke.

use std::path::{Component, Path, PathBuf};

/// Longest accepted skill name, matching the discovery-side limit.
pub const MAX_NAME_LEN: usize = 64;

/// A `SKILL.md` that passed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidSkill {
    /// Normalized skill name, used as the directory and command name.
    pub name: String,
    /// Description used for listings and for model-driven invocation.
    pub description: String,
}

/// Normalize a name into a skill slug.
///
/// Mirrors the discovery-side rule so a skill installed by the marketplace
/// resolves under the same name the agent later discovers: lowercase, every
/// non-alphanumeric character becomes a hyphen, runs collapse, and edges trim.
pub fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        let c = c.to_ascii_lowercase();
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '-'
        };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let trimmed = out.trim_matches('-');
    trimmed.chars().take(MAX_NAME_LEN).collect::<String>()
}

/// Whether a name is already a valid skill slug.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Validate a `SKILL.md` body, falling back to `fallback_name` when the
/// frontmatter omits `name`.
///
/// Returns a human-readable reason on failure so the CLI can explain which
/// skill in a multi-skill source was skipped and why.
pub fn validate_skill_md(
    contents: &str,
    fallback_name: Option<&str>,
) -> Result<ValidSkill, String> {
    let parsed = xai_grok_tools::implementations::skills::discovery::parse_skill_frontmatter(
        contents,
        fallback_name,
    )
    .map_err(|e| match e {
        xai_grok_tools::implementations::skills::discovery::SkillParseError::NoFrontmatter => {
            "SKILL.md has no YAML frontmatter".to_string()
        }
        xai_grok_tools::implementations::skills::discovery::SkillParseError::YamlError(msg) => {
            format!("SKILL.md frontmatter is not valid YAML: {msg}")
        }
        xai_grok_tools::implementations::skills::discovery::SkillParseError::InvalidName(name) => {
            format!("SKILL.md declares an unusable name `{name}`")
        }
    })?;

    let name = normalize_name(&parsed.name);
    if !is_valid_name(&name) {
        return Err(format!("skill name `{}` is not a usable slug", parsed.name));
    }

    // Discovery derives a missing description from the body's first prose
    // paragraph and, failing that, from the skill's own name. Validation
    // mirrors that so it never rejects a skill discovery would happily load;
    // a name-only description is weak, but it is the agent's problem to rank,
    // not a reason to refuse the install.
    let description = match parsed.description.trim() {
        "" => derive_description_from_body(contents).unwrap_or_else(|| name.clone()),
        text => text.to_string(),
    };

    Ok(ValidSkill { name, description })
}

/// Fall back to the body's leading prose the way skill discovery does.
fn derive_description_from_body(contents: &str) -> Option<String> {
    use xai_grok_tools::implementations::skills::discovery::{
        MAX_BODY_PEEK_BYTES, extract_first_paragraph,
    };

    let body = xai_grok_tools::implementations::skills::skill::extract_skill_body(contents);
    let peek = if body.len() > MAX_BODY_PEEK_BYTES {
        body.char_indices()
            .take_while(|(i, _)| *i <= MAX_BODY_PEEK_BYTES)
            .last()
            .map_or(body.as_str(), |(i, _)| &body[..i])
    } else {
        body.as_str()
    };
    extract_first_paragraph(peek).filter(|d| !d.trim().is_empty())
}

/// Join `relative` under `root`, rejecting anything that escapes it.
///
/// Purely lexical, so it works for paths that do not exist yet — which is the
/// case for every install destination before it is written.
pub fn join_within(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() {
        return None;
    }
    let normalized = relative.replace('\\', "/");
    let mut result = root.to_path_buf();
    let mut depth = 0usize;

    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str()?;
                if text.contains(':') {
                    return None;
                }
                result.push(text);
                depth += 1;
            }
            Component::CurDir => continue,
            // A parent component is only safe while it stays under `root`,
            // and an install path has no reason to use one at all.
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    (depth > 0).then_some(result)
}

#[cfg(test)]
#[path = "skill_validate_tests.rs"]
mod tests;
