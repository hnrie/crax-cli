use std::path::Path;

use super::{MAX_NAME_LEN, is_valid_name, join_within, normalize_name, validate_skill_md};

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n# Body\n")
}

#[test]
fn normalizes_mixed_case_and_separators() {
    assert_eq!(normalize_name("Deploy To Prod"), "deploy-to-prod");
    assert_eq!(normalize_name("review_pr"), "review-pr");
    assert_eq!(normalize_name("tool-v1.2"), "tool-v1-2");
    assert_eq!(normalize_name("  spaced  "), "spaced");
}

#[test]
fn normalization_collapses_and_trims_hyphens() {
    assert_eq!(normalize_name("--a---b--"), "a-b");
    assert_eq!(normalize_name("!!!"), "");
}

#[test]
fn normalization_caps_length() {
    let long = "a".repeat(200);
    assert_eq!(normalize_name(&long).len(), MAX_NAME_LEN);
}

#[test]
fn valid_names_are_lowercase_slugs() {
    assert!(is_valid_name("deploy"));
    assert!(is_valid_name("review-pr-2"));
    assert!(!is_valid_name(""));
    assert!(!is_valid_name("-lead"));
    assert!(!is_valid_name("trail-"));
    assert!(!is_valid_name("double--hyphen"));
    assert!(!is_valid_name("UPPER"));
    assert!(!is_valid_name("has space"));
    assert!(!is_valid_name(&"a".repeat(MAX_NAME_LEN + 1)));
}

#[test]
fn validates_a_well_formed_skill() {
    let parsed = validate_skill_md(&skill_md("deploy", "Ship the app"), None).unwrap();
    assert_eq!(parsed.name, "deploy");
    assert_eq!(parsed.description, "Ship the app");
}

#[test]
fn normalizes_the_frontmatter_name() {
    let parsed = validate_skill_md(&skill_md("Deploy To Prod", "Ship it"), None).unwrap();
    assert_eq!(parsed.name, "deploy-to-prod");
}

#[test]
fn falls_back_to_the_directory_name() {
    let contents = "---\ndescription: Ship the app\n---\n\nbody\n";
    let parsed = validate_skill_md(contents, Some("deploy")).unwrap();
    assert_eq!(parsed.name, "deploy");
}

#[test]
fn derives_a_description_from_the_body_when_absent() {
    let contents = "---\nname: deploy\n---\n\nShips the app to production.\n";
    let parsed = validate_skill_md(contents, None).unwrap();
    assert!(parsed.description.contains("Ships the app"));
}

#[test]
fn rejects_content_without_frontmatter() {
    let error = validate_skill_md("# Just markdown\n", Some("deploy")).unwrap_err();
    assert!(error.contains("frontmatter"), "unexpected error: {error}");
}

#[test]
fn rejects_a_skill_with_no_usable_name() {
    let contents = "---\nname: \"!!!\"\ndescription: Ship it\n---\n\nbody\n";
    assert!(validate_skill_md(contents, None).is_err());
}

#[test]
fn falls_back_to_the_skill_name_when_no_description_exists_anywhere() {
    // Discovery does the same, so validation must not reject a skill that
    // would otherwise load fine.
    let parsed = validate_skill_md("---\nname: deploy\n---\n", None).unwrap();
    assert_eq!(parsed.description, "deploy");
}

#[test]
fn prefers_frontmatter_description_over_the_body() {
    let contents = "---\nname: deploy\ndescription: From frontmatter\n---\n\nFrom body.\n";
    let parsed = validate_skill_md(contents, None).unwrap();
    assert_eq!(parsed.description, "From frontmatter");
}

#[test]
fn body_fallback_skips_a_leading_heading() {
    let contents = "---\nname: deploy\n---\n\n# Deploy Skill\n\nShips the app.\n";
    let parsed = validate_skill_md(contents, None).unwrap();
    assert_eq!(parsed.description, "Ships the app.");
}

#[test]
fn joins_relative_paths_under_the_root() {
    let root = Path::new("/base");
    assert_eq!(
        join_within(root, "SKILL.md").unwrap(),
        Path::new("/base/SKILL.md")
    );
    assert_eq!(
        join_within(root, "rules/a.md").unwrap(),
        Path::new("/base/rules/a.md")
    );
    assert_eq!(
        join_within(root, "./rules/a.md").unwrap(),
        Path::new("/base/rules/a.md")
    );
    assert_eq!(
        join_within(root, "rules\\a.md").unwrap(),
        Path::new("/base/rules/a.md")
    );
}

#[test]
fn rejects_paths_that_escape_the_root() {
    let root = Path::new("/base");
    assert_eq!(join_within(root, "../escape"), None);
    assert_eq!(join_within(root, "rules/../../escape"), None);
    assert_eq!(join_within(root, "/absolute"), None);
    assert_eq!(join_within(root, "C:/win"), None);
    assert_eq!(join_within(root, ""), None);
    assert_eq!(join_within(root, "."), None);
}
