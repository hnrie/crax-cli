use super::{
    DEFAULT_REGISTRY_URL, MAX_SEARCH_LIMIT, RegistryClient, RegistryError, RegistryFile,
    RegistrySkill, RegistrySkillFiles, is_safe_relative_path, split_registry_id,
    validate_skill_files,
};

fn files(paths: &[(&str, &str)]) -> RegistrySkillFiles {
    RegistrySkillFiles {
        files: paths
            .iter()
            .map(|(path, contents)| RegistryFile {
                path: (*path).to_string(),
                contents: (*contents).to_string(),
            })
            .collect(),
        hash: Some("abc123".into()),
    }
}

#[test]
fn default_base_url_is_the_public_registry() {
    let client = RegistryClient::with_base_url(DEFAULT_REGISTRY_URL);
    assert_eq!(client.base_url(), "https://skills.sh");
}

#[test]
fn base_url_trailing_slash_is_trimmed() {
    let client = RegistryClient::with_base_url("https://mirror.internal/");
    assert_eq!(client.base_url(), "https://mirror.internal");
}

#[tokio::test]
async fn search_rejects_single_character_queries_without_network() {
    let client = RegistryClient::with_base_url("http://127.0.0.1:1");
    assert!(matches!(
        client.search("a", 10, None).await.unwrap_err(),
        RegistryError::QueryTooShort
    ));
    assert!(matches!(
        client.search("  ", 10, None).await.unwrap_err(),
        RegistryError::QueryTooShort
    ));
}

#[test]
fn search_limit_is_clamped_to_the_documented_maximum() {
    assert_eq!(MAX_SEARCH_LIMIT, 200);
    assert_eq!(5000usize.clamp(1, MAX_SEARCH_LIMIT), 200);
    assert_eq!(0usize.clamp(1, MAX_SEARCH_LIMIT), 1);
}

#[test]
fn splits_three_segment_registry_ids() {
    assert_eq!(
        split_registry_id("vercel-labs/skills/find-skills"),
        Some((
            "vercel-labs".to_string(),
            "skills".to_string(),
            "find-skills".to_string()
        ))
    );
    assert_eq!(
        split_registry_id("/owner/repo/slug/"),
        Some(("owner".to_string(), "repo".to_string(), "slug".to_string()))
    );
}

#[test]
fn rejects_registry_ids_with_the_wrong_segment_count() {
    assert_eq!(split_registry_id("owner/repo"), None);
    assert_eq!(split_registry_id("owner/repo/slug/extra"), None);
    assert_eq!(split_registry_id(""), None);
}

#[tokio::test]
async fn download_by_id_rejects_malformed_ids_without_network() {
    let client = RegistryClient::with_base_url("http://127.0.0.1:1");
    assert!(matches!(
        client.download_by_id("owner/repo").await.unwrap_err(),
        RegistryError::NotFound(_)
    ));
}

#[test]
fn accepts_ordinary_relative_paths() {
    assert!(is_safe_relative_path("SKILL.md"));
    assert!(is_safe_relative_path("rules/deploy.md"));
    assert!(is_safe_relative_path("scripts/nested/run.sh"));
}

#[test]
fn rejects_absolute_traversal_and_prefixed_paths() {
    assert!(!is_safe_relative_path("/etc/passwd"));
    assert!(!is_safe_relative_path("../escape.md"));
    assert!(!is_safe_relative_path("rules/../../escape.md"));
    assert!(!is_safe_relative_path("C:/Windows/system32"));
    assert!(!is_safe_relative_path("..\\escape.md"));
    assert!(!is_safe_relative_path(""));
    assert!(!is_safe_relative_path("bad\u{0}name.md"));
    assert!(!is_safe_relative_path("./SKILL.md"));
}

#[test]
fn validation_accepts_a_well_formed_skill() {
    let payload = files(&[("SKILL.md", "---\nname: x\n---\nbody"), ("a/b.md", "x")]);
    assert!(validate_skill_files("owner/repo/x", &payload).is_ok());
}

#[test]
fn validation_rejects_a_skill_without_skill_md() {
    let payload = files(&[("README.md", "hello")]);
    assert!(matches!(
        validate_skill_files("owner/repo/x", &payload).unwrap_err(),
        RegistryError::Malformed(_)
    ));
}

#[test]
fn validation_rejects_an_empty_payload() {
    let payload = files(&[]);
    assert!(matches!(
        validate_skill_files("owner/repo/x", &payload).unwrap_err(),
        RegistryError::Malformed(_)
    ));
}

#[test]
fn validation_rejects_unsafe_paths() {
    let payload = files(&[("SKILL.md", "x"), ("../evil.md", "x")]);
    assert!(matches!(
        validate_skill_files("owner/repo/x", &payload).unwrap_err(),
        RegistryError::UnsafePath { .. }
    ));
}

#[test]
fn validation_rejects_oversized_files() {
    let huge = "a".repeat(3 * 1024 * 1024);
    let payload = files(&[("SKILL.md", "x"), ("big.md", huge.as_str())]);
    assert!(matches!(
        validate_skill_files("owner/repo/x", &payload).unwrap_err(),
        RegistryError::TooLarge { .. }
    ));
}

#[test]
fn skill_md_lookup_is_case_insensitive() {
    let payload = files(&[("skill.md", "body")]);
    assert!(payload.skill_md().is_some());
    assert_eq!(payload.total_bytes(), 4);
}

#[test]
fn registry_skill_exposes_page_url_and_install_spec() {
    let skill = RegistrySkill {
        id: "owner/repo/slug".into(),
        slug: "slug".into(),
        name: "Slug".into(),
        source: "owner/repo".into(),
        installs: 12,
    };
    assert_eq!(skill.install_spec(), "owner/repo/slug");
    assert!(skill.page_url().ends_with("/owner/repo/slug"));
}
